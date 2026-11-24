//! The map stage: one window of the slice becomes a Fallout 2 map and the
//! mod layer that carries it.
//!
//! It chains the three map crates, which do not know each other:
//! `map-slicer` turns placed objects into art, placements and ground
//! overlays; `map-terrain` turns the terrain plus those overlays into floor
//! tiles; `map-builder` assembles the map, the prototypes, the lists and
//! the layer, and parses everything back before returning.
//!
//! Output: `<out>/mods/mojave/` (the layer), `<out>/map-report.md`, and
//! `<out>/gallery/map-<slice>.png`, a composite of the map as the engine
//! draws it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use foundry_render::Projection;
use mir::{CellDoc, CellLocation, GroundSampler, MapWindow, Mir};
use quantize::PaletteMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vault_formats::geometry::HexGrid;

use crate::config::{ConvertError, Inputs};

/// Bumped whenever the stage's output for the same inputs would change.
const STAGE_VERSION: &str = "map-8";
/// Land textures are filtered one mip level finer than the pixel footprint
/// asks for: crisper cracks and grain, judged on engine frames.
pub(crate) const GROUND_LOD_BIAS: f32 = -1.0;
/// How far from a teleporting door (hex steps) the navmesh overrides a
/// building's bounding footprint.
pub(crate) const DOOR_APPROACH: i32 = 6;

/// The layer's id in `vault-mod.toml`. A re-run replaces a folder only when
/// it carries this id.
pub const LAYER_ID: &str = "mojave";

/// Art-list entries kept back for what the builder adds itself (the
/// invisible blocker, padding), in a run that extends the base lists. A
/// standalone run uses `map_builder::art_budget`'s exact count instead.
const BUILDER_RESERVE: usize = 8;
/// The terrain library cap of a run that extends the base tile list.
const EXTENDING_TILE_CAP: usize = 2000;

/// The map stage's own record of the largest things it placed, beside the
/// report: what `shots --auto` points the camera at.
pub const FEATURES_FILE: &str = "map-features.json";
/// A layer's per-map features: `<out>/features/<stem>.json`.
pub const FEATURES_DIR: &str = "features";
/// At most this many features are recorded.
const MAX_FEATURES: usize = 64;

/// What one run may spend: the slicer's budget, the terrain library cap,
/// and every art list as `map_builder::art_budget` sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Budgets {
    pub standalone: bool,
    pub slicer: map_slicer::Budget,
    pub tile_cap: usize,
    pub lists: BTreeMap<String, map_builder::ArtListBudget>,
}

/// The budgets for these base files and mode. A run that extends the base
/// lists keeps the numbers it always had: the free entries of each list
/// less [`BUILDER_RESERVE`], tiles capped at 2,000. A standalone run takes
/// what `art_budget` leaves the caller's art, all of it.
pub fn budgets(
    base: &BTreeMap<String, Vec<u8>>,
    standalone: bool,
) -> Result<Budgets, ConvertError> {
    let lists = map_builder::art_budget(base, standalone).map_err(stage)?;
    let list = |name: &str| {
        lists
            .get(name)
            .copied()
            .ok_or_else(|| stage(format!("the builder reports no {name} list")))
    };
    let (slicer, tile_cap) = if standalone {
        (
            map_slicer::Budget {
                walls: list("walls")?.available(),
                scenery: list("scenery")?.available(),
                items: list("items")?.available(),
            },
            list("tiles")?.available(),
        )
    } else {
        let kept_back = |name: &str| list(name).map(|l| l.free.saturating_sub(BUILDER_RESERVE));
        (
            map_slicer::Budget {
                walls: kept_back("walls")?,
                scenery: kept_back("scenery")?,
                items: kept_back("items")?,
            },
            kept_back("tiles")?.min(EXTENDING_TILE_CAP),
        )
    };
    Ok(Budgets {
        standalone,
        slicer,
        tile_cap,
        lists,
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapSummary {
    /// `true` when nothing changed and the existing layer was kept.
    pub reused: bool,
    pub map_file: String,
    pub map_number: u32,
    pub references: usize,
    pub art_entries: usize,
    pub placements: usize,
    pub floor_tiles: usize,
    pub walkable_hexes: usize,
    pub blockers: usize,
    pub exits: usize,
    pub maps: usize,
    pub layer_files: usize,
    pub critters: usize,
}

pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ConvertError + '_ {
    move |source| ConvertError::Io {
        path: path.to_owned(),
        source,
    }
}

pub(crate) fn stage(error: impl std::fmt::Display) -> ConvertError {
    ConvertError::stage("map", error)
}

pub(crate) fn slicer_category(category: map_slicer::ArtCategory) -> map_builder::ArtCategory {
    match category {
        map_slicer::ArtCategory::Walls => map_builder::ArtCategory::Walls,
        map_slicer::ArtCategory::Scenery => map_builder::ArtCategory::Scenery,
        map_slicer::ArtCategory::Items => map_builder::ArtCategory::Items,
    }
}

pub(crate) fn slicer_role(role: map_slicer::Role) -> map_builder::Role {
    match role {
        map_slicer::Role::WallStrip => map_builder::Role::WallStrip,
        map_slicer::Role::Scenery => map_builder::Role::Scenery,
        map_slicer::Role::Door => map_builder::Role::Door,
        map_slicer::Role::Container => map_builder::Role::Container,
        map_slicer::Role::Item => map_builder::Role::Item,
        map_slicer::Role::FloorPiece => map_builder::Role::FloorPiece,
    }
}

/// A map file stem the format accepts: at most eight of `[a-z0-9_-]`.
pub(crate) fn map_stem(slice: &str) -> String {
    let stem: String = slice
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .map(|c| c.to_ascii_lowercase())
        .take(8)
        .collect();
    if stem.is_empty() {
        "mojave".to_owned()
    } else {
        stem
    }
}

/// Everything the stage's output depends on, hashed.
pub(crate) fn cache_key(
    inputs: &Inputs,
    palette_bytes: &[u8],
    base: &BTreeMap<String, Vec<u8>>,
) -> Result<String, ConvertError> {
    let manifest_path = inputs.mir_dir().join("manifest.json");
    let manifest = std::fs::read(&manifest_path).map_err(io(&manifest_path))?;
    let mut hasher = Sha256::new();
    hasher.update(STAGE_VERSION.as_bytes());
    // The manifest names every plugin, mesh and texture by content hash.
    hasher.update(Sha256::digest(&manifest));
    hasher.update(inputs.calibration.to_toml().map_err(stage)?.as_bytes());
    hasher.update(Sha256::digest(palette_bytes));
    hasher.update(format!("{:?}", inputs.backend).as_bytes());
    hasher.update(inputs.slice.to_toml().as_bytes());
    // Only a standalone run hashes its mode, so that every key made before
    // the mode existed stays valid.
    if inputs.standalone {
        hasher.update(b"standalone");
    }
    // Likewise the game: hashed from Fallout 3 on, never for New Vegas (the
    // manifest names it too).
    if inputs.game != mir::GameVariant::FalloutNewVegas {
        hasher.update(inputs.game.id().as_bytes());
    }
    // And the declutter profile, only when on: the slice's own tables are
    // in its TOML above; `--declutter` is not.
    let declutter = inputs.declutter_profile();
    if declutter != "off" {
        hasher.update(format!("declutter={declutter}").as_bytes());
        // The preset itself, so that tuning it rebuilds.
        hasher.update(format!("{:?}", crate::declutter::readable_for(inputs.game)).as_bytes());
    }
    // Loot, only when on (the slice's own key is in its TOML above;
    // `--loot` is not): the effective rebalance tables, not the file.
    if inputs.loot_on() {
        // Bumped whenever the catalogue's output for the same tables changes.
        hasher.update(b"loot=on catalogue-3");
        hasher.update(inputs.rebalance()?.digest().as_bytes());
    }
    // The critter stage's index names every sprite set by content key.
    let critters = inputs.critters_dir().join("index.json");
    if let Ok(index) = std::fs::read(&critters) {
        hasher.update(Sha256::digest(&index));
    }
    for (path, bytes) in base {
        hasher.update(path.as_bytes());
        hasher.update(Sha256::digest(bytes));
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A person (or creature) of an interior the map stage did not hand to the
/// builder, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonSkip {
    pub name: String,
    pub reference: String,
    pub reason: String,
}

/// `name` after "a" or "an", by its first letter.
fn with_article(name: &str) -> String {
    let vowel = name
        .chars()
        .next()
        .is_some_and(|c| "AEIOUaeiou".contains(c));
    format!("{} {name}", if vowel { "an" } else { "a" })
}

/// The display name of an actor entry: its name, else its editor id.
pub(crate) fn person_name(entry: &crate::critters::CritterEntry) -> String {
    entry
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| entry.editor_id.clone())
        .unwrap_or_else(|| "Resident".to_owned())
}

/// The townsfolk the builder is given, and on an interior slice whoever
/// was left out before it.
pub(crate) type CritterInputs = (
    Vec<map_builder::CritterArt>,
    Vec<map_builder::CritterPlacement>,
    Vec<PersonSkip>,
);

/// The slice's townsfolk for the builder: the sprite sets the critter stage
/// rendered, and one placement per human reference in the window that
/// starts enabled (by its own flag and by its enable parent, the rule the
/// slicer applies to statics) and whose actor has a sprite set.
///
/// On an interior slice, `place_disabled` places people the data starts
/// disabled too; a person whose actor has fewer than 1 hit point is dead
/// in the data and left out (an exterior lifts it to 1); every person and
/// creature left out is returned with the reason. Several references to
/// one actor each get a placement.
fn critter_inputs(
    inputs: &Inputs,
    cells: &[CellDoc],
    references: &[mir::Reference],
) -> Result<CritterInputs, ConvertError> {
    let mut sets: BTreeMap<String, usize> = BTreeMap::new();
    let mut art = Vec::new();
    let (placements, skipped) =
        critter_inputs_into(inputs, cells, references, &mut sets, &mut art)?;
    Ok((art, placements, skipped))
}

/// [`critter_inputs`] into a layer's shared sprite sets: `sets` maps a set
/// key to its index in `art`, which a set the call needs is added to.
pub(crate) fn critter_inputs_into(
    inputs: &Inputs,
    cells: &[CellDoc],
    references: &[mir::Reference],
    sets: &mut BTreeMap<String, usize>,
    art: &mut Vec<map_builder::CritterArt>,
) -> Result<(Vec<map_builder::CritterPlacement>, Vec<PersonSkip>), ConvertError> {
    let interior = inputs.slice.is_interior();
    let place_disabled = interior && inputs.slice.place_disabled;
    let mut skipped: Vec<PersonSkip> = Vec::new();
    let index = crate::critters::read_index(inputs)?;
    let by_actor: BTreeMap<&str, &crate::critters::CritterEntry> =
        index.iter().map(|e| (e.actor.as_str(), e)).collect();
    let label = |reference: &mir::Reference| {
        by_actor
            .get(reference.base.to_string().as_str())
            .map(|e| person_name(e))
            .or_else(|| reference.base_editor_id.clone())
            .unwrap_or_else(|| reference.base.to_string())
    };
    let enabled: BTreeMap<String, bool> = cells
        .iter()
        .flat_map(|cell| cell.references.iter())
        .map(|r| {
            (
                r.form.to_string(),
                !(r.flags.deleted || r.flags.initially_disabled),
            )
        })
        .collect();
    let mut placements = Vec::new();
    for reference in references {
        let mut skip = |reason: String| {
            if interior {
                skipped.push(PersonSkip {
                    name: label(reference),
                    reference: reference.form.to_string(),
                    reason,
                });
            }
        };
        if reference.base_kind.as_deref() == Some("CREA") && !reference.flags.deleted {
            skip("a creature: creatures are not rendered yet".to_owned());
            continue;
        }
        if reference.base_kind.as_deref() != Some("NPC_") || reference.flags.deleted {
            continue;
        }
        if reference.flags.initially_disabled && !place_disabled {
            skip("starts disabled in the data (place_disabled is off)".to_owned());
            continue;
        }
        if let Some(parent) = &reference.enable_parent
            && !place_disabled
        {
            let parent_on = enabled
                .get(&parent.reference.to_string())
                .copied()
                .unwrap_or(true);
            if parent_on == parent.opposite {
                skip(format!(
                    "its enable parent {} starts it disabled (place_disabled is off)",
                    parent.reference
                ));
                continue;
            }
        }
        let Some(entry) = by_actor.get(reference.base.to_string().as_str()) else {
            skip("no actor document (run the critter stage)".to_owned());
            continue;
        };
        let Some(key) = &entry.set else {
            skip(format!(
                "no sprite set: {}",
                entry.error.as_deref().unwrap_or("not rendered")
            ));
            continue;
        };
        if interior && entry.health < 1 {
            skip(format!("dead in the data ({} hit points)", entry.health));
            continue;
        }
        let name = person_name(entry);
        let set = match sets.get(key) {
            Some(set) => *set,
            None => {
                art.push(map_builder::CritterArt {
                    label: name.clone(),
                    frms: crate::critters::read_set(inputs, key)?,
                });
                sets.insert(key.clone(), art.len() - 1);
                art.len() - 1
            }
        };
        let [x, y, _] = reference.transform.position;
        placements.push(map_builder::CritterPlacement {
            art: set,
            reference: reference.form.to_string(),
            description: format!("You see {name}."),
            name,
            position: [x, y],
            yaw: reference.transform.rotation[2],
            hit_points: entry.health.max(1),
            female: entry.female,
        });
    }
    if interior {
        // A name several people share is not a proper name: "You see a
        // Powder Ganger." (An exterior keeps its descriptions as they were.)
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for placement in &placements {
            *counts.entry(placement.name.clone()).or_default() += 1;
        }
        for placement in &mut placements {
            if counts[&placement.name] > 1 {
                placement.description = format!("You see {}.", with_article(&placement.name));
            }
        }
    }
    Ok((placements, skipped))
}

/// Paints the map as the engine draws it: floor tiles, then every object in
/// ascending hex order, each frame bottom-centre at its hex centre plus
/// shift and displacement.
pub(crate) fn compose_preview(
    ground: &map_terrain::Ground,
    sliced: &map_slicer::Sliced,
) -> (Vec<u8>, u32, u32) {
    let (origin, width, height) = map_terrain::Ground::bounds();
    let (mut canvas, _, _) = ground.compose();
    let grid = HexGrid::default();
    let mut order: Vec<&map_slicer::Placement> = sliced.placements.iter().collect();
    order.sort_by_key(|p| p.hex);
    for placement in order {
        let Some(art) = sliced.art.get(placement.art) else {
            continue;
        };
        let Ok(centre) = grid.hex_center(placement.hex) else {
            continue;
        };
        let x = centre[0] + i64::from(art.shift[0]) + i64::from(placement.displacement[0]);
        let bottom = centre[1] + i64::from(art.shift[1]) + i64::from(placement.displacement[1]);
        let left = x - i64::from(art.width / 2) - i64::from(origin[0]);
        let top = bottom - i64::from(art.height) + 1 - i64::from(origin[1]);
        for row in 0..i64::from(art.height) {
            let y = top + row;
            if y < 0 || y >= i64::from(height) {
                continue;
            }
            for column in 0..i64::from(art.width) {
                let px = left + column;
                if px < 0 || px >= i64::from(width) {
                    continue;
                }
                let pixel = art.pixels[(row * i64::from(art.width) + column) as usize];
                if pixel != 0 {
                    canvas[(y * i64::from(width) + px) as usize] = pixel;
                }
            }
        }
    }
    (canvas, width, height)
}

/// The base game's files the builder takes (`map_builder::base_file_paths`),
/// which the layer must ship whole. A synthetic install has none, and then
/// everything starts empty.
pub(crate) fn base_files(inputs: &Inputs) -> BTreeMap<String, Vec<u8>> {
    let mut base: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for path in map_builder::base_file_paths() {
        if let Ok(bytes) = inputs.f2_file(path) {
            base.insert((*path).to_owned(), bytes);
        }
    }
    base
}

/// The slicer's starting configuration for a game: its pattern profile.
///
/// New Vegas keeps `SlicerConfig::default()` exactly (the patterns were
/// tuned on the Mojave: `nv` and `scol` name prefixes, desert flora, rock
/// and canyon relief), so its maps keep their bytes. Fallout 3's
/// profile is tuned on the Capital Wasteland: no `nv` prefix; rubble and
/// litter piles flat or in relief instead of upright; dead trees and
/// stumps as single sprites; elevated highways and overpasses kept upright
/// (the `road` pattern would flatten them).
pub fn slicer_defaults(game: mir::GameVariant) -> map_slicer::SlicerConfig {
    let base = map_slicer::SlicerConfig::default();
    match game {
        mir::GameVariant::FalloutNewVegas => base,
        mir::GameVariant::Fallout3 => {
            let strings =
                |items: &[&str]| -> Vec<String> { items.iter().map(|s| (*s).to_owned()).collect() };
            map_slicer::SlicerConfig {
                relief_patterns: strings(&[
                    "rock",
                    "cliff",
                    "canyon",
                    "boulder",
                    "rubble",
                    "*rubblepile",
                    "rubchunk",
                    "!rocket",
                ]),
                flat_patterns: strings(&[
                    "road",
                    "*decal",
                    "*litter",
                    "tracks",
                    "parkinglot",
                    "*puddle",
                    "!roadsign",
                    "!roadblock",
                    "!overpass",
                    "!highway",
                ]),
                sprite_patterns: strings(&[
                    "trees\\", "plants\\", "*shrub", "*bush", "*stump", "treedead", "!ambush",
                ]),
                name_prefixes: strings(&["scol"]),
                ..base
            }
        }
    }
}

/// The map window and everything the slicer needs about it.
pub(crate) struct Scene {
    pub(crate) projection: Projection,
    pub(crate) window: MapWindow,
    pub(crate) worldspace: String,
    pub(crate) sampler: GroundSampler,
    pub(crate) cells: Vec<CellDoc>,
    pub(crate) references: Vec<mir::Reference>,
    pub(crate) build_options: map_builder::BuildOptions,
    pub(crate) slicer_config: map_slicer::SlicerConfig,
    pub(crate) open: OpenHexes,
    /// What an interior slice chose; `None` for an exterior window.
    pub(crate) interior: Option<InteriorScene>,
}

/// Where an interior slice's numbers came from, for the report.
#[derive(Debug, Clone)]
pub(crate) struct InteriorScene {
    /// The cell's editor id as the slice names it, and its form key.
    pub(crate) cell: String,
    pub(crate) form: String,
    pub(crate) floor_z: f32,
    pub(crate) floor_from: &'static str,
    pub(crate) storeys: Vec<mir::Storey>,
    pub(crate) centre: [f32; 2],
    pub(crate) centre_from: &'static str,
    /// The point the entrance is sought near, and what it is.
    pub(crate) entrance_near: Option<[f32; 2]>,
    pub(crate) entrance_from: String,
}

pub(crate) fn scene(inputs: &Inputs, mir: &Mir) -> Result<Scene, ConvertError> {
    if let Some(id) = &inputs.slice.interior {
        return interior_scene(inputs, mir, id);
    }
    let projection = Projection::new(&inputs.calibration).map_err(stage)?;
    let window = MapWindow::from_center(inputs.slice.center(), projection.tile_units());
    let worldspace = inputs.slice.worldspace.to_ascii_lowercase();
    let sampler = GroundSampler::load(mir, &worldspace, &window).map_err(stage)?;

    // Cells the window touches, with a margin so that an object standing
    // just outside still reaches in.
    let (xs, ys) = window.cells(1);
    let mut cells: Vec<CellDoc> = Vec::new();
    for y in ys {
        for x in xs.clone() {
            if let Some(doc) = mir.cell(&worldspace, x, y).map_err(stage)? {
                cells.push(doc);
            }
        }
    }
    if cells.is_empty() {
        return Err(stage(format!(
            "no extracted cell of {} lies in the map window; run extract first, or move map_center",
            inputs.slice.worldspace
        )));
    }
    // A reference belongs to the map when its origin is in the window. A
    // standalone run also takes a static whose origin is outside but whose
    // footprint reaches in (a control tower, a hotel wing): left out, the
    // ground it stands on shows, often painted black under it in the
    // source. Its pixels off the map are dropped by the slicer.
    let reaches_in = |r: &mir::Reference| {
        inputs.standalone
            && matches!(r.base_kind.as_deref(), Some("STAT" | "SCOL"))
            && footprint_reaches(mir, r, &window)
    };
    let references: Vec<mir::Reference> = cells
        .iter()
        .flat_map(|cell| cell.references.iter())
        .filter(|r| {
            window.contains(r.transform.position[0], r.transform.position[1]) || reaches_in(r)
        })
        .cloned()
        .collect();

    // Where the game's own navmesh lets an actor stand, a building's
    // bounding footprint must not block: porches, canopies, doorsteps.
    let build_options = map_builder::BuildOptions {
        standalone: inputs.standalone,
        ..map_builder::BuildOptions::default()
    };
    let frame = map_builder::walk::Frame::new(&window, &projection).map_err(stage)?;
    let standable = map_builder::walk::standable_hexes(&frame, &sampler, &cells, &build_options);
    let policy = OpenPolicy::of(inputs.standalone);
    let open_hexes: BTreeSet<i32> = match policy {
        OpenPolicy::Everywhere => standable.iter().copied().collect(),
        OpenPolicy::NearDoors => {
            let near_doors: BTreeSet<i32> = references
                .iter()
                .filter(|r| r.teleport.is_some())
                .filter_map(|r| {
                    let [x, y, _] = r.transform.position;
                    frame.hex_of_source(f64::from(x), f64::from(y))
                })
                .flat_map(|door| map_builder::exits::hexes_around(door, DOOR_APPROACH, false))
                .collect();
            standable
                .iter()
                .copied()
                .filter(|hex| near_doors.contains(hex))
                .collect()
        }
    };
    // Every hex the builder could ever make walkable (navmesh and slope,
    // and slope alone in cells without a navmesh), before anything blocks:
    // landmark pieces need not be cut for walkers who cannot exist.
    let (verdicts, _) = map_builder::walk::verdicts(&frame, &sampler, &cells, &[], &build_options);
    let landmark_walkers: BTreeSet<i32> = verdicts
        .iter()
        .enumerate()
        .filter(|(_, v)| **v == map_builder::walk::Verdict::Walkable)
        .map(|(hex, _)| hex as i32)
        .collect();
    let open = OpenHexes {
        policy,
        standable: standable.len(),
        opened: open_hexes.len(),
        walkers: landmark_walkers.len(),
    };
    let slicer_config = map_slicer::SlicerConfig {
        open_hexes,
        landmark_walkers: Some(landmark_walkers),
        ..slicer_defaults(inputs.game)
    };
    Ok(Scene {
        projection,
        window,
        worldspace,
        sampler,
        cells,
        references,
        build_options,
        slicer_config,
        open,
        interior: None,
    })
}

/// The scene of an interior slice (`SliceSpec::interior`): the one room,
/// on a flat floor, the window centred on it, every standable hex open.
pub(crate) fn interior_scene(inputs: &Inputs, mir: &Mir, id: &str) -> Result<Scene, ConvertError> {
    let slice = &inputs.slice;
    let projection = Projection::new(&inputs.calibration).map_err(stage)?;
    let doc = mir.interior(id).map_err(stage)?.ok_or_else(|| {
        let known: Vec<&str> = mir.interiors().map(|e| e.id.as_str()).collect();
        stage(format!(
            "interior {id} was not extracted: no door of the slice's cells leads to it within \
             interior_depth {} (extracted: {})",
            slice.interior_depth,
            if known.is_empty() {
                "none".to_owned()
            } else {
                known.join(", ")
            }
        ))
    })?;
    let cells = vec![doc];
    let storeys = mir::storeys(&cells[0]);
    let (floor_z, floor_from) = match (slice.floor_z, storeys.first()) {
        (Some(z), _) => (z, "the slice's floor_z"),
        (None, Some(storey)) => (storey.floor_z, "the largest navmesh storey"),
        (None, None) => (
            map_builder::dominant_floor_z(&cells).ok_or_else(|| {
                stage(format!(
                    "interior {id} has no navmesh to find its floor from; set floor_z"
                ))
            })?,
            "the dominant navmesh height",
        ),
    };
    let mut interior_build = slice.interior_build(floor_z);
    let (centre, centre_from) = match slice.map_center {
        Some(c) => (c, "the slice's map_center"),
        None => match map_builder::navmesh_centre(&cells, &interior_build) {
            Some(c) => (c, "the floor level's navmesh centre"),
            None => (
                reference_centre(&cells[0].references),
                "the centre of the room's references",
            ),
        },
    };
    let window = MapWindow::from_center(centre, projection.tile_units());
    let (entrance_near, entrance_from) = match slice.entrance {
        Some(p) => (Some(p), "the slice's entrance".to_owned()),
        None => match first_door(&cells[0].references) {
            Some(door) => (
                Some([door.transform.position[0], door.transform.position[1]]),
                format!("door {}, the room's first teleport door", door.form),
            ),
            None => (
                None,
                "the floor level's navmesh centre (no door)".to_owned(),
            ),
        },
    };
    interior_build.entrance_near = entrance_near;
    let sampler = GroundSampler::flat(floor_z);
    let references: Vec<mir::Reference> = cells[0]
        .references
        .iter()
        .filter(|r| {
            window.contains(r.transform.position[0], r.transform.position[1])
                || (matches!(r.base_kind.as_deref(), Some("STAT" | "SCOL"))
                    && footprint_reaches(mir, r, &window))
        })
        .cloned()
        .collect();
    let build_options = map_builder::BuildOptions {
        standalone: inputs.standalone,
        interior: Some(interior_build),
        ..map_builder::BuildOptions::default()
    };
    let frame = map_builder::walk::Frame::new(&window, &projection).map_err(stage)?;
    let standable = map_builder::walk::standable_hexes(&frame, &sampler, &cells, &build_options);
    let (verdicts, _) = map_builder::walk::verdicts(&frame, &sampler, &cells, &[], &build_options);
    let landmark_walkers: BTreeSet<i32> = verdicts
        .iter()
        .enumerate()
        .filter(|(_, v)| **v == map_builder::walk::Verdict::Walkable)
        .map(|(hex, _)| hex as i32)
        .collect();
    let open_hexes: BTreeSet<i32> = standable.iter().copied().collect();
    let open = OpenHexes {
        policy: OpenPolicy::Everywhere,
        standable: standable.len(),
        opened: open_hexes.len(),
        walkers: landmark_walkers.len(),
    };
    let slicer_config = map_slicer::SlicerConfig {
        open_hexes,
        landmark_walkers: Some(landmark_walkers),
        interior: Some(slice.interior_slice(floor_z)),
        ..slicer_defaults(inputs.game)
    };
    let interior = InteriorScene {
        cell: cells[0].editor_id.clone().unwrap_or_else(|| id.to_owned()),
        form: cells[0].form.to_string(),
        floor_z,
        floor_from,
        storeys,
        centre,
        centre_from,
        entrance_near,
        entrance_from,
    };
    Ok(Scene {
        projection,
        window,
        worldspace: slice.worldspace.to_ascii_lowercase(),
        sampler,
        cells,
        references,
        build_options,
        slicer_config,
        open,
        interior: Some(interior),
    })
}

/// What the declutter wiring chose for a run, for the report.
#[derive(Debug, Clone, Default)]
pub(crate) struct DeclutterChoice {
    /// The profile name (`off`, `readable`) and where it came from.
    pub(crate) profile: String,
    pub(crate) from: &'static str,
    /// References the quests and scripts point at, in the whole plugin
    /// list, and how many of them lie in the window.
    pub(crate) referenced: usize,
    pub(crate) referenced_here: usize,
    /// Keys handed to the builder as interactive.
    pub(crate) interactive: usize,
}

/// Sets the scene's declutter profile (`crate::declutter::profile`) and,
/// with it on, the builder's interactive set: the protected references
/// (quest and script targets) and every reference of the window whose
/// base kind is protected (by the profile, or by the default list, which
/// names every item kind) or that teleports. Off, nothing changes.
pub(crate) fn apply_declutter(
    inputs: &Inputs,
    mir: &Mir,
    scene: &mut Scene,
) -> Result<DeclutterChoice, ConvertError> {
    let profile = inputs.declutter_profile();
    let from = if inputs.declutter.is_some() {
        "--declutter"
    } else if inputs
        .slice
        .declutter
        .as_ref()
        .is_some_and(|d| d.profile.is_some())
    {
        "the slice's [declutter] profile"
    } else {
        "the game's default"
    };
    let mut choice = DeclutterChoice {
        profile: profile.clone(),
        from,
        ..DeclutterChoice::default()
    };
    if profile == "off" {
        // Validated all the same: tables without the pass are an error.
        crate::declutter::profile(
            inputs.game,
            &profile,
            inputs.slice.declutter.as_ref(),
            inputs.slice.tone.as_ref(),
            &BTreeSet::new(),
        )
        .map_err(|e| ConvertError::Slice {
            name: inputs.slice.name.clone(),
            reason: e,
        })?;
        return Ok(choice);
    }
    let referenced: BTreeSet<String> = mir
        .records()
        .map_err(stage)?
        .referenced_refs()
        .map_err(stage)?
        .iter()
        .map(ToString::to_string)
        .collect();
    let Some(declutter) = crate::declutter::profile(
        inputs.game,
        &profile,
        inputs.slice.declutter.as_ref(),
        inputs.slice.tone.as_ref(),
        &referenced,
    )
    .map_err(|e| ConvertError::Slice {
        name: inputs.slice.name.clone(),
        reason: e,
    })?
    else {
        return Ok(choice);
    };
    // Interactive whatever the slice protects: a bottle the clutter rule
    // may drop is still a thing to pick up when it stays.
    let interactive_kinds: BTreeSet<String> = declutter
        .protected_kinds
        .iter()
        .cloned()
        .chain(map_slicer::DeclutterProfile::default().protected_kinds)
        .collect();
    let mut interactive: BTreeSet<String> = BTreeSet::new();
    for r in &scene.references {
        let key = r.form.to_string();
        let kind_protected = r
            .base_kind
            .as_deref()
            .is_some_and(|k| interactive_kinds.contains(k));
        let referred = declutter.protected_refs.contains(&key);
        choice.referenced_here += usize::from(referenced.contains(&key));
        if kind_protected || referred || r.teleport.is_some() {
            interactive.insert(key);
        }
    }
    choice.referenced = referenced.len();
    choice.interactive = interactive.len();
    scene.build_options.interactive = interactive;
    scene.slicer_config.declutter = Some(declutter);
    Ok(choice)
}

/// The Fallout 3 shopping carts' model pattern, which the readable preset
/// prunes as clutter unless `wire = true` draws them solid.
pub(crate) const CART_PATTERN: &str = "*shoppingcart";

/// The wire rule of `wire = true`: the slicer's defaults, with shopping
/// carts named outright (their lattice is found by rendering too).
pub(crate) fn wire_rule() -> map_slicer::WireRule {
    map_slicer::WireRule {
        patterns: vec![CART_PATTERN.to_owned(), "*wirebasket".to_owned()],
        ..map_slicer::WireRule::default()
    }
}

/// The slice keys of the layers pass that the single-map path honours as
/// well, each only when present (absent, nothing changes): `wire`,
/// `navmesh_coverage`, `sight`, `entrance_sight`, and `entrance` on an
/// exterior.
pub(crate) fn apply_slice_keys(slice: &crate::SliceSpec, scene: &mut Scene) {
    if slice.wire == Some(true) {
        scene.slicer_config.wire = Some(wire_rule());
        // Drawn solid now: no longer pruned as clutter.
        if let Some(clutter) = scene
            .slicer_config
            .declutter
            .as_mut()
            .and_then(|d| d.clutter.as_mut())
        {
            clutter.patterns.retain(|p| p != CART_PATTERN);
        }
    }
    if slice.navmesh_coverage.is_some() {
        scene.build_options.navmesh_coverage = slice.coverage();
    }
    let (readability, entrance) = slice.sight_rules();
    if slice.sight.is_some() {
        scene.build_options.readability_sight = readability;
    }
    if slice.entrance_sight.is_some() {
        scene.build_options.entrance_sight = entrance;
    }
    if !slice.is_interior()
        && let Some(point) = slice.entrance
    {
        scene.build_options.entrance_near = Some(point);
    }
}

/// The terrain's tone: the profile's floor tone when the slice gives a
/// `[tone.floor]` (with the declutter pass on); otherwise none, and the
/// ground keeps its bytes.
pub(crate) fn terrain_tone(
    slice: &crate::SliceSpec,
    config: &map_slicer::SlicerConfig,
) -> Option<foundry_render::Tone> {
    slice.tone.as_ref().and_then(|t| t.floor)?;
    config.declutter.as_ref().map(|d| d.tones.floor)
}

/// The teleport door of `references` with the lowest form key: the door a
/// player would usually arrive by (the cell's first).
pub(crate) fn first_door(references: &[mir::Reference]) -> Option<&mir::Reference> {
    references
        .iter()
        .filter(|r| r.teleport.is_some() && !r.flags.deleted && r.transform.is_finite())
        .min_by(|a, b| a.form.cmp(&b.form))
}

/// The centre of the bounding box of the references' origins; the origin
/// when there are none.
fn reference_centre(references: &[mir::Reference]) -> [f32; 2] {
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for r in references.iter().filter(|r| r.transform.is_finite()) {
        for k in 0..2 {
            lo[k] = lo[k].min(r.transform.position[k]);
            hi[k] = hi[k].max(r.transform.position[k]);
        }
    }
    if lo[0] > hi[0] {
        return [0.0, 0.0];
    }
    [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]
}

/// Whether the bounding footprint of `reference`'s mesh, as placed,
/// overlaps the window (by the footprint's bounding rectangle).
fn footprint_reaches(mir: &Mir, reference: &mir::Reference, window: &MapWindow) -> bool {
    let Some(bounds) = reference
        .mesh
        .as_deref()
        .and_then(|hash| mir.manifest().mesh_info.get(hash))
        .and_then(|info| info.bounds)
    else {
        return false;
    };
    if !reference.transform.is_finite() {
        return false;
    }
    let footprint = map_slicer::Footprint::of(&reference.transform, bounds);
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &footprint.polygon {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let west = f64::from(window.x_west);
    let north = f64::from(window.y_north);
    let size = f64::from(window.size);
    lo[0] < west + size && hi[0] > west && lo[1] < north && hi[1] > north - size
}

/// Which standable hexes a building's bounding footprint leaves open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPolicy {
    /// Only those within [`DOOR_APPROACH`] steps of a teleporting door: a
    /// run that extends the base lists, whose walls list cannot afford the
    /// strips that no longer merge.
    NearDoors,
    /// Every one: a standalone run, with twice the walls list and the
    /// budget ladder.
    Everywhere,
}

impl OpenPolicy {
    pub fn of(standalone: bool) -> Self {
        if standalone {
            OpenPolicy::Everywhere
        } else {
            OpenPolicy::NearDoors
        }
    }
}

/// The open hexes of a run, counted, for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenHexes {
    pub policy: OpenPolicy,
    /// Hexes the navmesh and the slope limit let an actor stand on.
    pub standable: usize,
    /// Of those, the ones given to the slicer as open.
    pub opened: usize,
    /// Hexes given to the slicer as `landmark_walkers`.
    pub walkers: usize,
}

/// The hexes of the slicer's merged strip runs: every hex from a run's
/// first to its last, in its row. They hold wall art and must never be
/// opened by a passage (`BuildOptions::protected_hexes`). Sorted.
pub fn run_hexes(placements: &[map_slicer::Placement]) -> Vec<i32> {
    let hexes: BTreeSet<i32> = placements
        .iter()
        .filter_map(|p| p.run)
        .flat_map(|(a, b)| a.min(b)..=a.max(b))
        .collect();
    hexes.into_iter().collect()
}

/// What [`estimate`] found: the slicer's pre-check of the window against
/// the run's budgets, nothing rendered.
#[derive(Debug, Clone)]
pub struct EstimateSummary {
    pub slice: String,
    pub references: usize,
    pub budgets: Budgets,
    pub report: map_slicer::BudgetReport,
}

/// The slicer's fast pre-check (`map_slicer::estimate`) of the slice's
/// window against the budget of the chosen mode. Needs `extract`; renders
/// and writes nothing.
pub fn estimate(inputs: &Inputs) -> Result<EstimateSummary, ConvertError> {
    inputs.check_mode()?;
    let mir = Mir::open(&inputs.mir_dir()).map_err(stage)?;
    let base = base_files(inputs);
    let budgets = budgets(&base, inputs.standalone)?;
    let mut scene = scene(inputs, &mir)?;
    apply_declutter(inputs, &mir, &mut scene)?;
    let report = map_slicer::estimate(
        &mir,
        &scene.references,
        &scene.window,
        &scene.sampler,
        &scene.projection,
        &budgets.slicer,
        &scene.slicer_config,
    )
    .map_err(stage)?;
    Ok(EstimateSummary {
        slice: inputs.slice.name.clone(),
        references: scene.references.len(),
        budgets,
        report,
    })
}

impl std::fmt::Display for EstimateSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let report = &self.report;
        let mode = if self.budgets.standalone {
            "standalone: the walls and tiles lists replace Fallout 2's"
        } else {
            "extending Fallout 2's lists"
        };
        writeln!(
            f,
            "estimate for {} ({mode}): {} references in the window",
            self.slice, self.references
        )?;
        writeln!(
            f,
            "  {:<8}  {:>6}  {:>6}  verdict",
            "list", "demand", "free"
        )?;
        for c in &report.categories {
            let headroom = c.headroom();
            let verdict = if headroom < 0 {
                format!("OVER by {}", -headroom)
            } else {
                format!("under by {headroom}")
            };
            writeln!(
                f,
                "  {:<8}  {:>6}  {:>6}  {verdict}",
                c.category.name(),
                c.used,
                c.free
            )?;
        }
        writeln!(
            f,
            "  {:<8}  {:>6}  {:>6}  terrain library cap",
            "tiles", "-", self.budgets.tile_cap
        )?;
        let classes: Vec<String> = report
            .classes
            .iter()
            .filter(|(_, count)| count.references > 0)
            .map(|(kind, count)| format!("{} {}", kind.name(), count.references))
            .collect();
        writeln!(f, "references by class: {}", classes.join(", "))?;
        writeln!(
            f,
            "landmarks: {} references in about {} pieces (scenery list)",
            report.landmarks.references, report.landmarks.pieces
        )?;
        for category in map_slicer::ArtCategory::ALL {
            let rows: Vec<&map_slicer::Contributor> = report
                .contributors
                .iter()
                .filter(|c| c.category == category)
                .take(8)
                .collect();
            if rows.is_empty() {
                continue;
            }
            writeln!(f, "largest {} contributors:", category.name())?;
            for c in rows {
                writeln!(
                    f,
                    "  {:>6} entries  {:>4} refs  {:<13}  {}",
                    c.entries,
                    c.references,
                    c.class.name(),
                    c.model
                )?;
            }
        }
        if report.fits() {
            write!(
                f,
                "fits: the map stage should not need the budget ladder (an estimate)"
            )
        } else {
            write!(
                f,
                "over: the map stage will climb the budget ladder; it fails only if the last step does not fit"
            )
        }
    }
}

/// One of the largest things on the map, for pointing a camera at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    /// `landmark` (bands in the scenery list) or `walls` (wall strips).
    pub kind: String,
    /// The reference's form key.
    pub reference: String,
    /// The normalised model path of its art.
    pub model: String,
    /// The hex under the centre of its drawn extent.
    pub hex: i32,
    /// Placements (bands or strips).
    pub pieces: usize,
    /// Opaque pixels of its art, as placed.
    pub pixels: u64,
}

/// `map-features.json`: the exterior map and its largest features, largest
/// first. On an interior demo also the room and the people placed in it,
/// for aiming frames at them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapFeatures {
    pub map_file: String,
    pub entry_hex: i32,
    pub features: Vec<Feature>,
    /// The interior cell's editor id; absent on an exterior map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interior: Option<String>,
    /// The people placed, ranked by [`rank_people`]; absent on an
    /// exterior map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<Person>,
    /// The source game (`mir::GameVariant::id`); absent in a file written
    /// before both games were converted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
}

/// A person placed on the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    /// The reference's form key.
    pub reference: String,
    pub hex: i32,
}

/// Orders people for the camera: those with a name of their own first
/// (Sunny Smiles before the fourth Powder Ganger), then by name, then by
/// hex. Applying it twice changes nothing.
pub fn rank_people(people: &mut [Person]) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for person in people.iter() {
        *counts.entry(person.name.clone()).or_default() += 1;
    }
    people.sort_by(|a, b| {
        (counts[&a.name] > 1, &a.name, a.hex).cmp(&(counts[&b.name] > 1, &b.name, b.hex))
    });
}

/// The people as features the shots stage aims at: kind `person`, the
/// name as the model, so that a shared name is shot again only after every
/// other.
pub fn people_features(people: &[Person]) -> Vec<Feature> {
    people
        .iter()
        .map(|p| Feature {
            kind: "person".to_owned(),
            reference: p.reference.clone(),
            model: p.name.clone(),
            hex: p.hex,
            pieces: 1,
            pixels: 0,
        })
        .collect()
}

impl MapFeatures {
    /// Reads `<out>/map-features.json`.
    pub fn read(out: &Path) -> Result<Self, ConvertError> {
        Self::read_path(&out.join(FEATURES_FILE))
    }

    /// The features of map `map` (a file name under `maps/`): a layer's
    /// `<out>/features/<stem>.json`, else `map-features.json` when it is
    /// that map's.
    pub fn read_for(out: &Path, map: &str) -> Result<Self, ConvertError> {
        let file = map.trim_start_matches("maps/");
        let stem = file.trim_end_matches(".map");
        let per_map = out.join(FEATURES_DIR).join(format!("{stem}.json"));
        if per_map.is_file() {
            return Self::read_path(&per_map);
        }
        let features = Self::read(out)?;
        if features.map_file.trim_start_matches("maps/") == file {
            Ok(features)
        } else {
            Err(stage(format!(
                "no features of {file}: {} is {}'s and {} does not exist",
                FEATURES_FILE,
                features.map_file,
                per_map.display()
            )))
        }
    }

    fn read_path(path: &Path) -> Result<Self, ConvertError> {
        let path = path.to_owned();
        let text = std::fs::read_to_string(&path).map_err(io(&path))?;
        serde_json::from_str(&text)
            .map_err(|e| ConvertError::stage("map", format!("{}: {e}", path.display())))
    }
}

/// The largest landmarks and wall-strip objects of a slicer run. A
/// landmark is a reference placed as more than one scenery piece, or whose
/// model the report counts as a landmark; a walls feature is a reference
/// placed as wall strips.
pub fn features(sliced: &map_slicer::Sliced) -> Vec<Feature> {
    let landmark_models: BTreeSet<&str> = sliced
        .report
        .contributors
        .iter()
        .filter(|c| c.class == map_slicer::ClassKind::Landmark)
        .map(|c| c.model.as_str())
        .collect();
    features_of(&sliced.art, &sliced.placements, &landmark_models)
}

fn features_of(
    art: &[map_slicer::ArtImage],
    placements: &[map_slicer::Placement],
    landmark_models: &BTreeSet<&str>,
) -> Vec<Feature> {
    #[derive(Default)]
    struct Group {
        walls: usize,
        scenery: usize,
        pixels: u64,
        model: String,
        bounds: Option<[i64; 4]>,
        /// Each placement's hex and the centre of its drawn extent.
        anchors: Vec<(i32, [i64; 2])>,
    }
    let grid = HexGrid::default();
    let mut groups: BTreeMap<&str, Group> = BTreeMap::new();
    for placement in placements {
        let wall = match placement.role {
            map_slicer::Role::WallStrip => true,
            map_slicer::Role::Scenery => false,
            _ => continue,
        };
        let (Some(art), Ok(centre)) = (art.get(placement.art), grid.hex_center(placement.hex))
        else {
            continue;
        };
        let group = groups.entry(placement.reference.as_str()).or_default();
        if wall {
            group.walls += 1;
        } else {
            group.scenery += 1;
        }
        if group.model.is_empty() {
            group.model = art.source.clone();
        }
        group.pixels += art.pixels.iter().filter(|p| **p != 0).count() as u64;
        let x = centre[0] + i64::from(art.shift[0]) + i64::from(placement.displacement[0]);
        let bottom = centre[1] + i64::from(art.shift[1]) + i64::from(placement.displacement[1]);
        let left = x - i64::from(art.width / 2);
        let top = bottom - i64::from(art.height) + 1;
        let right = left + i64::from(art.width);
        group
            .anchors
            .push((placement.hex, [(left + right) / 2, (top + bottom) / 2]));
        let b = group.bounds.get_or_insert([left, top, right, bottom]);
        *b = [
            b[0].min(left),
            b[1].min(top),
            b[2].max(right),
            b[3].max(bottom),
        ];
    }
    let mut out: Vec<Feature> = groups
        .into_iter()
        .filter_map(|(reference, group)| {
            let kind = if group.walls > 0 {
                "walls"
            } else if group.scenery > 1 || landmark_models.contains(group.model.as_str()) {
                "landmark"
            } else {
                return None;
            };
            let [l, t, r, b] = group.bounds?;
            let centre = [(l + r) / 2, (t + b) / 2];
            // A landmark cut off by the map's edge has the centre of its
            // drawn extent off the grid: aim at the piece nearest it.
            let hex = i32::try_from(centre[0])
                .ok()
                .zip(i32::try_from(centre[1]).ok())
                .and_then(|(x, y)| grid.pick_hex([x, y]))
                .or_else(|| {
                    group
                        .anchors
                        .iter()
                        .min_by_key(|(hex, c)| {
                            ((c[0] - centre[0]).pow(2) + (c[1] - centre[1]).pow(2), *hex)
                        })
                        .map(|(hex, _)| *hex)
                })?;
            Some(Feature {
                kind: kind.to_owned(),
                reference: reference.to_owned(),
                model: group.model,
                hex,
                pieces: group.walls + group.scenery,
                pixels: group.pixels,
            })
        })
        .collect();
    rank_features(&mut out);
    out.truncate(MAX_FEATURES);
    out
}

/// How many of the largest landmarks lead the features list.
pub const LEADING_LANDMARKS: usize = 4;

/// Orders features as `map-features.json` lists them: the
/// [`LEADING_LANDMARKS`] largest landmarks first, so that a long run of
/// fences, kerbs or crates never pushes a tower out of the list, then
/// everything else by placed pixels, largest first. A model the budget
/// ladder turned into a landmark (a retaining wall) ranks by its pixels
/// like any building after those. Applying it twice changes nothing.
pub fn rank_features(features: &mut [Feature]) {
    let by_pixels = |a: &Feature, b: &Feature| {
        b.pixels
            .cmp(&a.pixels)
            .then_with(|| a.reference.cmp(&b.reference))
    };
    features.sort_by(by_pixels);
    let leading: BTreeSet<String> = features
        .iter()
        .filter(|f| f.kind == "landmark")
        .take(LEADING_LANDMARKS)
        .map(|f| f.reference.clone())
        .collect();
    features.sort_by(|a, b| {
        leading
            .contains(&b.reference)
            .cmp(&leading.contains(&a.reference))
            .then_with(|| by_pixels(a, b))
    });
}

/// Builds the slice's map and writes the mod layer.
pub fn map(inputs: &Inputs) -> Result<MapSummary, ConvertError> {
    inputs.check_mode()?;
    // Several maps, storeys or levels: the layer path (`crate::layer`).
    if let Some(layer) = inputs.layer_spec() {
        return crate::layer::map_layer(inputs, &layer);
    }
    let mir = Mir::open(&inputs.mir_dir()).map_err(stage)?;
    let palette_bytes = inputs.f2_file("color.pal")?;
    let palette = inputs.palette()?;
    let palette_map = PaletteMap::try_new(&palette).map_err(stage)?;
    let base = base_files(inputs);

    let mod_dir = inputs.mod_dir();
    let key = cache_key(inputs, &palette_bytes, &base)?;
    let key_path = inputs.out.join("cache").join("map.key");
    let summary_path = inputs.out.join("cache").join("map.summary");
    let features_path = inputs.out.join(FEATURES_FILE);
    if mod_dir.join("vault-mod.toml").is_file()
        && features_path.is_file()
        && std::fs::read_to_string(&key_path).is_ok_and(|stored| stored == key)
        && let Ok(text) = std::fs::read_to_string(&summary_path)
        && let Some(summary) = MapSummary::from_cache(&text)
    {
        return Ok(MapSummary {
            reused: true,
            ..summary
        });
    }

    let budgets = budgets(&base, inputs.standalone)?;
    let mut scene = scene(inputs, &mir)?;
    let declutter_choice = apply_declutter(inputs, &mir, &mut scene)?;
    // `footprints = true`: the slicer finds the furniture and its placed
    // footprints, the builder blocks the hexes under them.
    let footprints_on = inputs.slice.footprints == Some(true);
    if footprints_on {
        scene.slicer_config.furniture = Some(map_slicer::FurnitureRule::default());
    }
    apply_slice_keys(&inputs.slice, &mut scene);
    let ground_tone = terrain_tone(&inputs.slice, &scene.slicer_config);
    let Scene {
        projection,
        window,
        worldspace,
        sampler,
        cells,
        references,
        mut build_options,
        slicer_config,
        open,
        interior,
    } = scene;

    // Interiors the window's doors open on, for the exits. An interior
    // demo has no exits.
    let mut interiors: BTreeMap<String, CellDoc> = BTreeMap::new();
    for reference in references.iter().filter(|_| interior.is_none()) {
        let Some(teleport) = &reference.teleport else {
            continue;
        };
        let Some(cell) = &teleport.cell else { continue };
        if !matches!(cell.location, CellLocation::Interior) {
            continue;
        }
        let key = cell.form.to_string();
        if !interiors.contains_key(&key)
            && let Some(doc) = mir.interior_by_form(&cell.form).map_err(stage)?
        {
            interiors.insert(key, doc);
        }
    }

    let mut renderer =
        foundry_render::renderer(inputs.backend, &inputs.calibration).map_err(stage)?;
    // A budget overflow arrives here as an error whose message carries the
    // list, the cap, the demand and the largest contributors.
    let sliced = map_slicer::slice_statics(
        &mir,
        &references,
        &window,
        &sampler,
        &projection,
        renderer.as_mut(),
        &palette_map,
        &budgets.slicer,
        &slicer_config,
    )
    .map_err(stage)?;
    // A passage must never open a hex that wall art stands on. The builder
    // finds most of them itself; the slicer's merged runs are named here.
    // (The slicer does not report a landmark's blocked core apart from
    // relief, which passages may cross, so that core is left to the
    // builder's own test.)
    build_options.protected_hexes = run_hexes(&sliced.placements);
    let protected = build_options.protected_hexes.len();
    if footprints_on {
        build_options.footprints = sliced.footprints.clone();
    }

    let overlays: Vec<map_terrain::Overlay<'_>> = sliced
        .overlays
        .iter()
        .map(|o| map_terrain::Overlay {
            origin: o.origin,
            width: o.width,
            height: o.height,
            rgba: &o.rgba,
        })
        .collect();
    // A standalone run has about four times the tiles; greedy merging
    // spends them better than k-means at every cap. A run that extends the
    // base list keeps the default method, and so its bytes.
    let terrain_options = map_terrain::TerrainOptions {
        library_cap: budgets.tile_cap,
        library_method: if inputs.standalone {
            map_terrain::LibraryMethod::GreedyMerge
        } else {
            map_terrain::TerrainOptions::default().library_method
        },
        jobs: inputs.jobs,
        texture_lod_bias: GROUND_LOD_BIAS,
        // Indoors only the baked floor is drawn; the rest stays black.
        blank_without_land: interior.is_some(),
        tone: ground_tone,
        ..map_terrain::TerrainOptions::default()
    };
    let ground = if interior.is_some() {
        // A room has no land: its coordinates would read the worldspace's.
        map_terrain::build_ground_from(
            &map_terrain::MemoryLand::default(),
            &window,
            &sampler,
            &projection,
            &inputs.calibration,
            &palette_map,
            &overlays,
            &terrain_options,
        )
    } else {
        map_terrain::build_ground(
            &mir,
            &worldspace,
            &window,
            &sampler,
            &projection,
            &inputs.calibration,
            &palette_map,
            &overlays,
            &terrain_options,
        )
    }
    .map_err(stage)?;
    // Blank squares get the engine's blank tile, not a library tile.
    let squares: Vec<u16> = ground
        .squares
        .iter()
        .enumerate()
        .map(|(q, tile)| {
            if ground.is_blank(q) {
                map_builder::BLANK_SQUARE
            } else {
                *tile
            }
        })
        .collect();

    let art: Vec<map_builder::ArtImage> = sliced
        .art
        .iter()
        .map(|a| map_builder::ArtImage {
            category: slicer_category(a.category),
            width: a.width,
            height: a.height,
            pixels: a.pixels.clone(),
            shift: a.shift,
            source: a.source.clone(),
        })
        .collect();
    let placements: Vec<map_builder::Placement> = sliced
        .placements
        .iter()
        .map(|p| map_builder::Placement {
            art: p.art,
            hex: p.hex,
            displacement: p.displacement,
            role: slicer_role(p.role),
            reference: p.reference.clone(),
            blocking: p.blocking,
        })
        .collect();
    let (critter_art, critters, people_skipped) = critter_inputs(inputs, &cells, &references)?;
    // Items, loot, armed and sided people (`crate::catalogue`), when on.
    let catalogue = if inputs.loot_on() {
        let rebalance = inputs.rebalance()?;
        let built = crate::catalogue::build(&crate::catalogue::CatalogueInput {
            inputs,
            mir: &mir,
            rebalance: &rebalance,
            references: &references,
            art: &sliced.art,
            placements: &sliced.placements,
            critters: &critters,
            base: &base,
            palette_map: &palette_map,
            palette_bytes: &palette_bytes,
        })?;
        build_options.catalogue = Some(std::sync::Arc::new(built.catalogue));
        Some(built.report)
    } else {
        None
    };
    let mut builder_cells = cells.clone();
    builder_cells.extend(interiors.into_values());
    let stem = inputs.slice.map_stem();
    let layer_name = format!("Mojave Foundry: {}", inputs.slice.name);
    let built = map_builder::build_map(&map_builder::BuildInput {
        map_name: &stem,
        map_number: None,
        layer_id: LAYER_ID,
        layer_name: &layer_name,
        window,
        sampler: &sampler,
        projection: &projection,
        ground: map_builder::GroundTiles {
            tiles: &ground.tiles,
            squares: &squares,
        },
        art: &art,
        placements: &placements,
        blocked_hexes: &sliced.blocked_hexes,
        critter_art: &critter_art,
        critters: &critters,
        cells: &builder_cells,
        base_files: &base,
        options: build_options,
    })
    .map_err(stage)?;

    if let Some(parent) = mod_dir.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    built.layer.write(&mod_dir).map_err(stage)?;
    // What is on disk is what a player loads: check that, not the copy in
    // memory.
    let written = map_builder::read_layer(&mod_dir).map_err(stage)?;
    let verified = map_builder::verify_layer(&written, &base).map_err(stage)?;

    // The preview and the report sit beside the layer, never inside it.
    let (indexed, width, height) = compose_preview(&ground, &sliced);
    let rgba = quantize::to_rgba(&indexed, &palette);
    let gallery_dir = inputs.gallery_dir();
    std::fs::create_dir_all(&gallery_dir).map_err(io(&gallery_dir))?;
    let preview_path = gallery_dir.join(format!("map-{stem}.png"));
    let png = foundry_gallery::encode_png(&rgba, width, height).map_err(stage)?;
    std::fs::write(&preview_path, png).map_err(io(&preview_path))?;
    if let Some(catalogue) = &catalogue {
        crate::gallery::items_page(inputs, &catalogue.items, &palette)?;
    }

    let report = &built.report;
    let summary = MapSummary {
        reused: false,
        map_file: report.map_file.clone(),
        map_number: report.map_number,
        references: references.len(),
        art_entries: art.len(),
        placements: placements.len(),
        floor_tiles: ground.tiles.len(),
        walkable_hexes: report.walk.walkable,
        blockers: report.blockers,
        exits: report.exits.len(),
        maps: report.maps.len(),
        layer_files: verified.frm
            + verified.art_lists
            + verified.prototype_lists
            + verified.prototypes
            + verified.messages
            + verified.maps.len(),
        critters: report.critters.len(),
    };

    let mut text = String::new();
    let _ = writeln!(text, "# Map report: {}\n", inputs.slice.name);
    let _ = writeln!(
        text,
        "Game: {} (`{}`); slicer pattern profile `{}`.\n",
        inputs.game.display_name(),
        inputs.game.id(),
        crate::source::short_id(inputs.game)
    );
    let _ = writeln!(
        text,
        "Window: {} units square, north-west corner at ({}, {}).\n",
        window.size, window.x_west, window.y_north
    );
    if let Some(scene) = &interior {
        write_interior_section(
            &mut text,
            scene,
            &sliced,
            report,
            &critters,
            &people_skipped,
            &references,
        );
    }
    let _ = writeln!(text, "## Art budget\n\n```\n{}\n```\n", sliced.report);
    write_list_section(&mut text, &budgets, report, &verified);
    write_slicer_sections(&mut text, &sliced);
    write_ground_section(&mut text, &ground.stats);
    write_open_section(&mut text, &open, protected);
    let _ = writeln!(text, "## Walkability\n\n```\n{:#?}\n```\n", report.walk);
    write_reach_sections(&mut text, report);
    write_declutter_section(&mut text, &declutter_choice, &sliced);
    let _ = writeln!(text, "## Readability\n");
    for line in report.readability_summary() {
        let _ = writeln!(text, "- {line}");
    }
    let _ = writeln!(text);
    write_loot_section(&mut text, built.loot.as_ref(), catalogue.as_ref());
    let _ = writeln!(text, "## Furniture\n");
    if !footprints_on {
        let _ = writeln!(
            text,
            "- footprints off (slice key `footprints = true` turns them on)"
        );
    }
    for line in built.furniture.summary() {
        let _ = writeln!(text, "- {line}");
    }
    let _ = writeln!(text);
    let _ = writeln!(
        text,
        "## Map\n\n- file: `{}`, number {}\n- entry hex: {}\n- objects: {}, of which blockers: {}\n- maps in the layer: {:?}\n",
        report.map_file,
        report.map_number,
        report.entry_hex,
        report.objects,
        report.blockers,
        report.maps
    );
    let _ = writeln!(text, "## Exits\n");
    for exit in &report.exits {
        let _ = writeln!(
            text,
            "- door {} at hex {} leads to `{}` (map {}); grids {} step{} out on {:?}; returns to hex {}",
            exit.door,
            exit.door_hex,
            exit.stub_file,
            exit.stub_number,
            exit.grid_steps,
            if exit.grid_steps == 1 { "" } else { "s" },
            exit.grid_hexes,
            exit.return_hex
        );
    }
    for skipped in &report.skipped_teleports {
        let _ = writeln!(
            text,
            "- door {} not converted: {}",
            skipped.door, skipped.reason
        );
    }
    let _ = writeln!(text, "\n## Characters\n");
    let _ = writeln!(
        text,
        "- {} placed from {} sprite sets ({} prototypes); AI packet {:?}, team {:?}",
        report.critters.len(),
        report.critter_sets.len(),
        report.critter_prototypes.len(),
        report.critter_ai_packet,
        report.critter_team
    );
    write_critter_art(&mut text, report, &critter_art);
    for critter in &report.critters {
        let _ = writeln!(
            text,
            "- {} ({}) on hex {}, facing {}, moved {} steps",
            critters[critter.index].name,
            critter.reference,
            critter.hex,
            critter.orientation,
            critter.steps
        );
    }
    for skipped in &report.skipped_critters {
        let _ = writeln!(
            text,
            "- {} ({}) not placed: {}",
            critters[skipped.index].name, skipped.reference, skipped.reason
        );
    }
    let _ = writeln!(text, "\n## Notes\n");
    for note in &report.notes {
        let _ = writeln!(text, "- {note}");
    }
    let _ = writeln!(
        text,
        "\n## Parsed back through vault-formats\n\n```\n{verified:#?}\n```"
    );
    let report_path = inputs.out.join("map-report.md");
    std::fs::write(&report_path, text).map_err(io(&report_path))?;

    let mut people: Vec<Person> = Vec::new();
    if interior.is_some() {
        people = report
            .critters
            .iter()
            .map(|c| Person {
                name: critters[c.index].name.clone(),
                reference: c.reference.clone(),
                hex: c.hex,
            })
            .collect();
        rank_people(&mut people);
    }
    let map_features = MapFeatures {
        map_file: report.map_file.clone(),
        entry_hex: report.entry_hex,
        features: features(&sliced),
        interior: interior.as_ref().map(|s| s.cell.clone()),
        people,
        game: Some(inputs.game.id().to_owned()),
    };
    let json = serde_json::to_string_pretty(&map_features).map_err(stage)?;
    std::fs::write(&features_path, json + "\n").map_err(io(&features_path))?;

    let cache_dir = inputs.out.join("cache");
    std::fs::create_dir_all(&cache_dir).map_err(io(&cache_dir))?;
    std::fs::write(&summary_path, summary.to_cache()).map_err(io(&summary_path))?;
    std::fs::write(&key_path, key).map_err(io(&key_path))?;
    Ok(summary)
}

/// `## Loot`: the builder's `loot_summary`, the catalogue's own lines and
/// the people's sides and kit.
pub(crate) fn write_loot_section(
    text: &mut String,
    loot: Option<&map_builder::LootReport>,
    catalogue: Option<&crate::catalogue::CatalogueReport>,
) {
    let _ = writeln!(text, "## Loot\n");
    let (Some(loot), Some(catalogue)) = (loot, catalogue) else {
        let _ = writeln!(
            text,
            "- off: no item prototypes, containers stay empty, people unarmed (slice key \
             `loot = true` or `--loot on`)\n"
        );
        return;
    };
    for line in loot.loot_summary() {
        let _ = writeln!(text, "- {line}");
    }
    for line in catalogue.lines() {
        let _ = writeln!(text, "- {line}");
    }
    let hostile: Vec<&str> = catalogue
        .people
        .iter()
        .filter(|p| p.side.hostile)
        .map(|p| p.name.as_str())
        .collect();
    let friendly: Vec<&str> = catalogue
        .people
        .iter()
        .filter(|p| !p.side.hostile)
        .map(|p| p.name.as_str())
        .collect();
    let _ = writeln!(
        text,
        "\nHostile ({}): {}.\n\nFriendly ({}): {}.\n",
        hostile.len(),
        if hostile.is_empty() {
            "none".to_owned()
        } else {
            hostile.join(", ")
        },
        friendly.len(),
        if friendly.is_empty() {
            "none".to_owned()
        } else {
            friendly.join(", ")
        }
    );
    let _ = writeln!(text, "{}", catalogue.people_table());
    let _ = writeln!(text, "| item | kind | found | figures |\n|---|---|---|---|");
    for item in &catalogue.items {
        let _ = writeln!(
            text,
            "| {} | {} | {} | {} |",
            item.name, item.kind, item.found, item.summary
        );
    }
    let _ = writeln!(text);
}

/// The critter art the layer ships: per sprite set its row, files and
/// bytes (every animation file of the set, weapon letters too), and the
/// rows of `critters.lst` used against its 4,096.
pub(crate) fn write_critter_art(
    text: &mut String,
    report: &map_builder::BuildReport,
    art: &[map_builder::CritterArt],
) {
    let mut files = 0;
    let mut bytes = 0u64;
    let mut highest = None;
    for set in &report.critter_sets {
        let frms = art.get(set.art).map(|a| &a.frms);
        let set_bytes: u64 = frms.map_or(0, |f| f.values().map(|b| b.len() as u64).sum());
        files += set.files.len();
        bytes += set_bytes;
        highest = highest.max(Some(set.row));
        let _ = writeln!(
            text,
            "- art: {} `{}` row {}: {} files, {:.1} MB",
            set.label,
            set.stem,
            set.row,
            set.files.len(),
            set_bytes as f64 / 1e6
        );
    }
    if let Some(row) = highest {
        let _ = writeln!(
            text,
            "- critter art: {} sets, {files} files, {:.1} MB; the highest row of \
             `art/critters/critters.lst` is {row} of the 4,096 the format addresses ({} free)",
            report.critter_sets.len(),
            bytes as f64 / 1e6,
            4095u32.saturating_sub(u32::from(row))
        );
    }
}

/// `## Declutter`: the profile and where it came from, then what the pass
/// did: pruned by rule, kept and pruned by kind, protected by reason, the
/// cutaway, and the open floor before and after.
pub(crate) fn write_declutter_section(
    text: &mut String,
    choice: &DeclutterChoice,
    sliced: &map_slicer::Sliced,
) {
    let _ = writeln!(text, "## Declutter\n");
    let _ = writeln!(
        text,
        "- profile: `{}`, from {}",
        choice.profile, choice.from
    );
    let Some(stats) = &sliced.report.declutter else {
        let _ = writeln!(
            text,
            "- the pass is off: nothing pruned, cut, toned or outlined\n"
        );
        return;
    };
    let _ = writeln!(
        text,
        "- quest and script references: {} in the plugin list, {} in the window; {} references handed to the builder as interactive",
        choice.referenced, choice.referenced_here, choice.interactive
    );
    let _ = writeln!(
        text,
        "- pruned: {} references; protected: {}; decals faded: {}; shells cut lower: {} ({} kept their cut for wall art); outlined sprites: {}",
        stats.pruned_total(),
        stats.protected_references,
        stats.decals_faded,
        stats.shell_cut_references,
        stats.shells_kept_for_art,
        stats.outlined
    );
    for (rule, count) in &stats.pruned {
        let _ = writeln!(text, "  - {rule}: {count}");
    }
    let protected: Vec<String> = stats
        .protected
        .iter()
        .map(|(reason, n)| format!("{reason} {n}"))
        .collect();
    let _ = writeln!(text, "- protected by reason: {}", protected.join(", "));
    let c = &stats.cutaway;
    let _ = writeln!(
        text,
        "- cutaway: {} of {} strips cut to the kerb ({} emptied, {} pixels removed); walkers hidden {} before, {} after",
        c.strips_cut,
        c.strips_considered,
        c.strips_emptied,
        c.pixels_removed,
        c.walkers_covered_before,
        c.walkers_covered_after
    );
    if let Some(before) = &stats.floor_before {
        let _ = writeln!(text, "- floor before the pass: {before}");
    }
    if let Some(after) = &stats.floor_after {
        let _ = writeln!(text, "- floor after the pass: {after}");
    }
    let _ = writeln!(text, "\n| kind | kept | pruned |\n|---|---:|---:|");
    for (kind, tally) in &stats.by_kind {
        let _ = writeln!(text, "| {kind} | {} | {} |", tally.kept, tally.pruned);
    }
    let _ = writeln!(text);
}

/// `## Interior`: the room, its floor and storeys, the cut, what the slicer
/// and the builder did indoors, the doors not converted, and every person
/// placed or left out with the reason.
pub(crate) fn write_interior_section(
    text: &mut String,
    scene: &InteriorScene,
    sliced: &map_slicer::Sliced,
    report: &map_builder::BuildReport,
    critters: &[map_builder::CritterPlacement],
    skipped: &[PersonSkip],
    references: &[mir::Reference],
) {
    let _ = writeln!(text, "## Interior\n");
    let _ = writeln!(
        text,
        "- cell: {} ({}); {} references in the window; a standalone demo map, one storey, \
         doors drawn but leading nowhere, no interior lighting",
        scene.cell,
        scene.form,
        references.len()
    );
    let _ = writeln!(
        text,
        "- floor: z {:.1}, from {}",
        scene.floor_z, scene.floor_from
    );
    if let Some(build) = &report.interior {
        let _ = writeln!(
            text,
            "- walkable band (navmesh): z {:.0} to {:.0}",
            build.band[0], build.band[1]
        );
    }
    let _ = writeln!(
        text,
        "- window centre: ({:.0}, {:.0}), from {}",
        scene.centre[0], scene.centre[1], scene.centre_from
    );
    match scene.entrance_near {
        Some(p) => {
            let _ = writeln!(
                text,
                "- entrance sought near ({:.0}, {:.0}): {}",
                p[0], p[1], scene.entrance_from
            );
        }
        None => {
            let _ = writeln!(text, "- entrance sought near {}", scene.entrance_from);
        }
    }
    let _ = writeln!(text, "- storeys found: {}", scene.storeys.len());
    for (n, s) in scene.storeys.iter().enumerate() {
        let chosen = if s.contains_z(scene.floor_z) || (s.floor_z - scene.floor_z).abs() < 1.0 {
            " (holds the chosen floor)"
        } else {
            ""
        };
        let _ = writeln!(
            text,
            "  {}. floor z {:.1}, z {:.0} to {:.0}, {:.0} square units, {} triangles, \
             x {:.0} to {:.0}, y {:.0} to {:.0}{chosen}",
            n + 1,
            s.floor_z,
            s.z_range[0],
            s.z_range[1],
            s.area,
            s.triangles,
            s.min[0],
            s.max[0],
            s.min[1],
            s.max[1]
        );
    }
    match &sliced.report.interior {
        Some(i) => {
            let s = &i.settings;
            let _ = writeln!(
                text,
                "- cut: {} above the floor (z {}); floor band {} to {}; hanging above {}",
                s.cut_height,
                s.cut_z(),
                s.floor_band[0],
                s.floor_band[1],
                s.hang_clearance
            );
            let _ = writeln!(
                text,
                "- floor baked: {} triangles from {} references ({} floor only), {} hexes, \
                 {:.0} square units, {} overlays",
                i.floor_triangles,
                i.floor_references,
                i.floor_only,
                i.floor_hexes.len(),
                i.floor_area,
                i.floor_overlays
            );
            let _ = writeln!(
                text,
                "- ceiling cut: {} references clipped, {} triangles over the cut, {} ceiling \
                 triangles dropped; left out: {} above the cut, {} hanging, {} below the floor",
                i.clipped_references,
                i.cut_triangles,
                i.ceiling_triangles,
                i.above_cut,
                i.hanging,
                i.below_floor
            );
        }
        None => {
            let _ = writeln!(text, "- the slicer reported no interior statistics");
        }
    }
    let _ = writeln!(text, "\nBuilder:\n");
    for line in report.interior_summary() {
        let _ = writeln!(text, "- {line}");
    }
    let _ = writeln!(text, "\nPeople placed: {}\n", report.critters.len());
    for critter in &report.critters {
        let _ = writeln!(
            text,
            "- {} ({}) on hex {}",
            critters[critter.index].name, critter.reference, critter.hex
        );
    }
    let skipped_count = skipped.len() + report.skipped_critters.len();
    let _ = writeln!(text, "\nPeople and creatures left out: {skipped_count}\n");
    for person in skipped {
        let _ = writeln!(
            text,
            "- {} ({}): {}",
            person.name, person.reference, person.reason
        );
    }
    for critter in &report.skipped_critters {
        let _ = writeln!(
            text,
            "- {} ({}): {}",
            critters[critter.index].name, critter.reference, critter.reason
        );
    }
    let _ = writeln!(text);
}

/// `## Art lists`: the mode, and every list the build loaded with its
/// free entries before and after.
pub(crate) fn write_list_section(
    text: &mut String,
    budgets: &Budgets,
    report: &map_builder::BuildReport,
    verified: &map_builder::VerifyReport,
) {
    let _ = writeln!(text, "## Art lists\n");
    let _ = writeln!(
        text,
        "Mode: {}.\n",
        if budgets.standalone {
            "standalone; the layer's walls and tiles lists replace Fallout 2's, so stock maps \
             draw the wrong walls and floors while it is mounted"
        } else {
            "extending Fallout 2's lists"
        }
    );
    let _ = writeln!(
        text,
        "Budget given: walls {}, scenery {}, items {}; terrain library cap {}.\n",
        budgets.slicer.walls, budgets.slicer.scenery, budgets.slicer.items, budgets.tile_cap
    );
    let _ = writeln!(
        text,
        "| list | replaced | base entries | kept | added | free before | free after | shipped |\n|---|---|---|---|---|---|---|---|"
    );
    for (name, list) in &report.art_lists {
        let _ = writeln!(
            text,
            "| {name} | {} | {} | {} | {} | {} | {} | {} |",
            if list.replaced { "yes" } else { "no" },
            list.base_entries,
            list.kept,
            list.added,
            list.free_before,
            list.free_after,
            if list.shipped { "yes" } else { "no" }
        );
    }
    let _ = writeln!(
        text,
        "\nReplaced lists the self-check accepted: {}.\n",
        if verified.replaced_art_lists.is_empty() {
            "none".to_owned()
        } else {
            verified.replaced_art_lists.join(", ")
        }
    );
}

/// `## Budget ladder`, `## Landmarks` and `## Trigger volumes skipped`.
pub(crate) fn write_slicer_sections(text: &mut String, sliced: &map_slicer::Sliced) {
    let report = &sliced.report;
    let _ = writeln!(text, "## Budget ladder\n");
    if report.ladder.is_empty() {
        let _ = writeln!(text, "Not climbed: the window fit at once.\n");
    }
    for (n, step) in report.ladder.iter().enumerate() {
        let _ = writeln!(
            text,
            "{}. {} ({}): walls {} -> {}, scenery {} -> {}, items {} -> {}; {} references, {} models",
            n + 1,
            step.kind.name(),
            step.change,
            step.before[0],
            step.after[0],
            step.before[1],
            step.after[1],
            step.before[2],
            step.after[2],
            step.references,
            step.models.len()
        );
    }
    if !report.ladder.is_empty() {
        let _ = writeln!(text);
    }
    let landmarks = &report.landmarks;
    let _ = writeln!(text, "## Landmarks\n");
    let _ = writeln!(
        text,
        "{} references placed in {} pieces (scenery list); {} baked as relief instead.\n",
        landmarks.references,
        landmarks.pieces,
        landmarks.fallbacks.len()
    );
    let _ = writeln!(
        text,
        "- pieces of the visibility cut: {}\n- visibility splits: {}\n- sorting splits: {}\n\
         - unresolved pieces: {}\n- walkers beside a landmark: {}, still hidden behind a piece: {}\n\
         - hexes sealed: {}\n",
        landmarks.tiles,
        landmarks.visibility_splits,
        landmarks.sorting_splits,
        landmarks.unresolved_pieces,
        landmarks.walkers,
        landmarks.hidden_walkers,
        landmarks.sealed_hexes
    );
    for fallback in &landmarks.fallbacks {
        let _ = writeln!(
            text,
            "- relief instead: {} `{}` ({})",
            fallback.reference, fallback.model, fallback.reason
        );
    }
    let triggers: Vec<&map_slicer::Skipped> = sliced
        .skipped
        .iter()
        .filter(|s| s.reason == map_slicer::SkipReason::TriggerVolume)
        .collect();
    let _ = writeln!(text, "\n## Trigger volumes skipped\n");
    let _ = writeln!(
        text,
        "{} references: invisible trigger or activator volumes, never drawn.",
        triggers.len()
    );
    for skipped in &triggers {
        let _ = writeln!(
            text,
            "- {} `{}`",
            skipped.reference,
            skipped.model.as_deref().unwrap_or("(no model)")
        );
    }
    let _ = writeln!(text);
}

/// `## Ground`: the default-texture and missing-texture tallies, then
/// every statistic.
pub(crate) fn write_ground_section(text: &mut String, stats: &map_terrain::GroundStats) {
    let _ = writeln!(text, "## Ground\n");
    let _ = writeln!(
        text,
        "- tiles: {} distinct of {} squares, {} in the library; {} squares substituted\n\
         - default land texture: {} layers, {} pixels\n\
         - missing textures: {} layers ({} naming no file), {} pixels; {} bases borrowed from a neighbour\n\
         - ground with no land: {} pixels\n",
        stats.distinct_tiles,
        stats.tiles_before,
        stats.tiles_after,
        stats.substituted_squares,
        stats.defaulted_texture_layers,
        stats.defaulted_texture_pixels,
        stats.missing_texture_layers,
        stats.unnamed_texture_layers,
        stats.missing_texture_pixels,
        stats.borrowed_base_layers,
        stats.fallback_pixels
    );
    // The stage timings would make the report differ run to run.
    let stats: Vec<String> = format!("{stats:#?}")
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .split(':')
                .next()
                .is_some_and(|k| k.ends_with("_time"))
        })
        .map(str::to_owned)
        .collect();
    let _ = writeln!(text, "```\n{}\n```\n", stats.join("\n"));
}

/// `## Open hexes`: which standable hexes a bounding footprint left open,
/// and what the slicer and the passages were told.
pub(crate) fn write_open_section(text: &mut String, open: &OpenHexes, protected: usize) {
    let _ = writeln!(text, "## Open hexes\n");
    let policy = match open.policy {
        OpenPolicy::Everywhere => "every standable hex (standalone)".to_owned(),
        OpenPolicy::NearDoors => {
            format!("standable hexes within {DOOR_APPROACH} steps of a teleporting door")
        }
    };
    let _ = writeln!(
        text,
        "- policy: {policy}\n- standable: {}, open: {}\n- landmark walkers given to the slicer: {}\n\
         - hexes protected from passages (merged strip runs): {protected}\n",
        open.standable, open.opened, open.walkers
    );
}

/// `## Entrance`, `## Components`, `## Passages` and `## Islands left`.
pub(crate) fn write_reach_sections(text: &mut String, report: &map_builder::BuildReport) {
    let entrance = &report.entrance;
    let _ = writeln!(
        text,
        "## Entrance\n\n- hex {}: {:?} (how a figure there is drawn against the art)\n\
         - in the largest component: {}\n- {} steps from the walkable hex nearest the window centre\n",
        entrance.hex,
        entrance.sight,
        if entrance.in_main { "yes" } else { "no" },
        entrance.steps_from_centre
    );
    let components = &report.components;
    let total: usize = components.sizes.iter().sum();
    let main = components
        .main
        .and_then(|m| components.sizes.get(m))
        .copied()
        .unwrap_or(0);
    let largest: Vec<String> = components
        .sizes
        .iter()
        .take(8)
        .map(|s| s.to_string())
        .collect();
    let _ = writeln!(
        text,
        "## Components\n\n- {} components, {} hexes in all; the entrance's holds {} ({:.1} %)\n\
         - largest: {}\n- unreachable exits: {} of {}; stranded returns: {}\n\
         - critters outside the entrance's component: {}\n- hexes where the sealing border leaks: {}\n",
        components.sizes.len(),
        total,
        main,
        if total == 0 {
            0.0
        } else {
            100.0 * main as f64 / total as f64
        },
        if largest.is_empty() {
            "none".to_owned()
        } else {
            largest.join(", ")
        },
        components.unreachable_exits(),
        components.exits.len(),
        components.stranded_returns(),
        components
            .critters
            .iter()
            .filter(|c| c.component.is_none() || c.component != components.main)
            .count(),
        components.leaked
    );
    let _ = writeln!(
        text,
        "## Passages\n\n{} opened, {} hexes made walkable.\n",
        report.passages.len(),
        report.opened_hexes
    );
    for passage in &report.passages {
        let _ = writeln!(
            text,
            "- {} to {}: {} hexes ({} off the navmesh), {} opened; joins an island of {} at {} holding {}",
            passage.from,
            passage.to,
            passage.length,
            passage.off_navmesh,
            passage.opened.len(),
            passage.island_size,
            passage.island_hex,
            passage.holds.join(", ")
        );
    }
    let _ = writeln!(text, "\n## Islands left\n");
    if report.islands_left.is_empty() {
        let _ = writeln!(text, "None.");
    }
    for island in &report.islands_left {
        let _ = writeln!(
            text,
            "- {} hexes at {} holding {}: {}",
            island.island_size,
            island.island_hex,
            island.holds.join(", "),
            island.reason
        );
    }
    let _ = writeln!(text);
}

impl MapSummary {
    pub(crate) fn to_cache(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.map_file,
            self.map_number,
            self.references,
            self.art_entries,
            self.placements,
            self.floor_tiles,
            self.walkable_hexes,
            self.blockers,
            self.exits,
            self.maps,
            self.layer_files,
            self.critters
        )
    }

    pub(crate) fn from_cache(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let map_file = lines.next()?.to_owned();
        let mut number = || lines.next()?.parse::<usize>().ok();
        Some(Self {
            reused: false,
            map_file,
            map_number: number()? as u32,
            references: number()?,
            art_entries: number()?,
            placements: number()?,
            floor_tiles: number()?,
            walkable_hexes: number()?,
            blockers: number()?,
            exits: number()?,
            maps: number()?,
            layer_files: number()?,
            critters: number()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic art list of `n` entries.
    fn list(n: usize, stem: &str) -> Vec<u8> {
        (0..n)
            .flat_map(|i| format!("{stem}{i:05}.frm\r\n").into_bytes())
            .collect()
    }

    fn base() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("art/walls/walls.lst".to_owned(), list(1670, "w")),
            ("art/scenery/scenery.lst".to_owned(), list(1860, "s")),
            ("art/items/items.lst".to_owned(), list(300, "i")),
            ("art/tiles/tiles.lst".to_owned(), list(3119, "t")),
        ])
    }

    /// What every run computed before standalone mode existed.
    fn old_free_entries(base: &BTreeMap<String, Vec<u8>>, name: &str) -> usize {
        let category = vault_formats::fid::ART_DIRECTORIES
            .iter()
            .position(|d| *d == name)
            .expect("an art directory") as u8;
        let list = match base.get(&format!("art/{name}/{name}.lst")) {
            Some(bytes) => emit::ArtList::from_base(category, bytes),
            None => emit::ArtList::new(category),
        }
        .expect("a list");
        list.remaining().saturating_sub(BUILDER_RESERVE)
    }

    #[test]
    fn an_extending_run_keeps_the_budget_it_always_had() {
        for base in [base(), BTreeMap::new()] {
            let budgets = budgets(&base, false).expect("budgets");
            assert!(!budgets.standalone);
            assert_eq!(
                budgets.slicer,
                map_slicer::Budget {
                    walls: old_free_entries(&base, "walls"),
                    scenery: old_free_entries(&base, "scenery"),
                    items: old_free_entries(&base, "items"),
                }
            );
            assert_eq!(budgets.tile_cap, old_free_entries(&base, "tiles").min(2000));
        }
    }

    #[test]
    fn a_standalone_run_has_nearly_every_wall_and_tile_entry() {
        let base = base();
        let extending = budgets(&base, false).expect("budgets");
        let standalone = budgets(&base, true).expect("budgets");
        assert_eq!(standalone.slicer.walls, 4096);
        assert_eq!(standalone.tile_cap, 4094, "tiles 0 and 1 stay reserved");
        assert!(standalone.lists["walls"].replaced && standalone.lists["tiles"].replaced);
        assert!(!standalone.lists["scenery"].replaced);
        // Scenery and items still extend the base: only the builder's own
        // entry is held back.
        assert_eq!(
            standalone.slicer.scenery,
            extending.lists["scenery"].free - 1
        );
        assert_eq!(
            standalone.slicer.walls - extending.slicer.walls,
            1670 + BUILDER_RESERVE
        );
    }

    fn image(width: u32, height: u32, source: &str) -> map_slicer::ArtImage {
        map_slicer::ArtImage {
            category: map_slicer::ArtCategory::Walls,
            width,
            height,
            pixels: vec![7; (width * height) as usize],
            shift: [0, 0],
            source: source.to_owned(),
        }
    }

    fn placement(
        art: usize,
        hex: i32,
        role: map_slicer::Role,
        reference: &str,
    ) -> map_slicer::Placement {
        map_slicer::Placement {
            art,
            hex,
            displacement: [0, 0],
            role,
            reference: reference.to_owned(),
            blocking: true,
            run: None,
        }
    }

    #[test]
    fn features_rank_landmarks_and_buildings_by_pixels() {
        use map_slicer::Role;
        let art = vec![
            image(32, 40, "house.nif"),
            image(200, 300, "tower.nif"),
            image(20, 20, "barrel.nif"),
        ];
        let placements = vec![
            placement(0, 20_100, Role::WallStrip, "a:house"),
            placement(0, 20_102, Role::WallStrip, "a:house"),
            placement(1, 10_100, Role::Scenery, "a:tower"),
            placement(1, 10_500, Role::Scenery, "a:tower"),
            // A single sprite is not a feature.
            placement(2, 30_000, Role::Scenery, "a:barrel"),
            placement(2, 30_010, Role::Door, "a:door"),
        ];
        let found = features_of(&art, &placements, &BTreeSet::new());
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].kind, "landmark");
        assert_eq!(found[0].reference, "a:tower");
        assert_eq!(found[0].pieces, 2);
        assert_eq!(found[0].pixels, 2 * 200 * 300);
        assert_eq!(found[1].kind, "walls");
        assert_eq!(found[1].pixels, 2 * 32 * 40);
        // The house's centre is between and above its two strips.
        let [c, r] = HexGrid::coordinates(found[1].hex).expect("on the grid");
        assert!((100..=102).contains(&c), "{c}");
        assert!(r <= 100, "{r}");
        // A one-piece scenery sprite counts when the report calls its
        // model a landmark.
        let one = vec![placement(1, 10_100, Role::Scenery, "a:tower")];
        assert!(features_of(&art, &one, &BTreeSet::new()).is_empty());
        let found = features_of(&art, &one, &BTreeSet::from(["tower.nif"]));
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn landmarks_rank_first_and_one_cut_off_by_the_edge_stays() {
        use map_slicer::Role;
        let art = vec![image(32, 40, "fence.nif"), image(40, 300, "tower.nif")];
        let mut placements: Vec<map_slicer::Placement> = (0..40)
            .map(|i| placement(0, 20_000 + i, Role::WallStrip, "a:fence"))
            .collect();
        // A tall landmark in the grid's first rows: the centre of its drawn
        // extent is above the grid.
        placements.push(placement(1, 3 * 200 + 100, Role::Scenery, "a:tower"));
        placements.push(placement(1, 3 * 200 + 102, Role::Scenery, "a:tower"));
        let mut found = features_of(&art, &placements, &BTreeSet::new());
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].pixels < found[1].pixels);
        let again = found.clone();
        rank_features(&mut found);
        assert_eq!(found, again, "ranking twice changes nothing");
        assert_eq!(found[0].kind, "landmark");
        assert!(
            [3 * 200 + 100, 3 * 200 + 102].contains(&found[0].hex),
            "the piece nearest the centre: {}",
            found[0].hex
        );
        assert_eq!(found[1].kind, "walls");
    }

    #[test]
    fn only_the_largest_landmarks_lead_the_buildings() {
        let feature = |reference: &str, kind: &str, pixels: u64| Feature {
            kind: kind.to_owned(),
            reference: reference.to_owned(),
            model: format!("{reference}.nif"),
            hex: 20_100,
            pieces: 2,
            pixels,
        };
        let mut features: Vec<Feature> = (0..6)
            .map(|i| feature(&format!("wall{i}"), "landmark", 100 + i))
            .collect();
        features.push(feature("casino", "walls", 50_000));
        features.push(feature("fence", "walls", 10));
        rank_features(&mut features);
        let order: Vec<&str> = features.iter().map(|f| f.reference.as_str()).collect();
        assert_eq!(
            order,
            [
                "wall5", "wall4", "wall3", "wall2", "casino", "wall1", "wall0", "fence"
            ]
        );
    }

    #[test]
    fn run_hexes_cover_each_merged_run_whole() {
        use map_slicer::Role;
        let mut a = placement(0, 1_005, Role::WallStrip, "a:house");
        a.run = Some((1_003, 1_007));
        let mut b = placement(0, 2_001, Role::WallStrip, "a:house");
        b.run = Some((2_001, 2_002));
        let c = placement(0, 3_000, Role::WallStrip, "a:house");
        assert_eq!(
            run_hexes(&[a, b, c]),
            [1_003, 1_004, 1_005, 1_006, 1_007, 2_001, 2_002]
        );
    }

    #[test]
    fn a_shared_name_takes_an_article() {
        assert_eq!(with_article("Powder Ganger"), "a Powder Ganger");
        assert_eq!(with_article("Ultra-Luxe Guard"), "an Ultra-Luxe Guard");
    }

    #[test]
    fn people_with_a_name_of_their_own_lead() {
        let person = |name: &str, hex: i32| Person {
            name: name.to_owned(),
            reference: format!("a:{hex}"),
            hex,
        };
        let mut people = vec![
            person("Powder Ganger", 300),
            person("Trudy", 900),
            person("Powder Ganger", 100),
            person("Sunny Smiles", 500),
        ];
        rank_people(&mut people);
        let order: Vec<(&str, i32)> = people.iter().map(|p| (p.name.as_str(), p.hex)).collect();
        assert_eq!(
            order,
            [
                ("Sunny Smiles", 500),
                ("Trudy", 900),
                ("Powder Ganger", 100),
                ("Powder Ganger", 300)
            ]
        );
        let again = people.clone();
        rank_people(&mut people);
        assert_eq!(people, again);
        let features = people_features(&people);
        assert_eq!(features[0].kind, "person");
        assert_eq!(features[0].model, "Sunny Smiles");
        assert_eq!(features[0].hex, 500);
    }

    #[test]
    fn only_a_standalone_run_opens_every_standable_hex() {
        assert_eq!(OpenPolicy::of(true), OpenPolicy::Everywhere);
        assert_eq!(OpenPolicy::of(false), OpenPolicy::NearDoors);
    }
}
