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
use sha2::{Digest, Sha256};
use vault_formats::geometry::HexGrid;

use crate::config::{ConvertError, Inputs};

/// Bumped whenever the stage's output for the same inputs would change.
const STAGE_VERSION: &str = "map-6";
/// Land textures are filtered one mip level finer than the pixel footprint
/// asks for: crisper cracks and grain, judged on engine frames.
const GROUND_LOD_BIAS: f32 = -1.0;
/// How far from a teleporting door (hex steps) the navmesh overrides a
/// building's bounding footprint.
const DOOR_APPROACH: i32 = 6;

/// The layer's id in `vault-mod.toml`. A re-run replaces a folder only when
/// it carries this id.
pub const LAYER_ID: &str = "mojave";

/// Art-list entries kept back for what the builder adds itself (the
/// invisible blocker, padding).
const BUILDER_RESERVE: usize = 8;

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

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ConvertError + '_ {
    move |source| ConvertError::Io {
        path: path.to_owned(),
        source,
    }
}

fn stage(error: impl std::fmt::Display) -> ConvertError {
    ConvertError::stage("map", error)
}

/// The art directory number of a category name, from the engine's own
/// table.
fn art_category(name: &str) -> Result<u8, ConvertError> {
    vault_formats::fid::ART_DIRECTORIES
        .iter()
        .position(|d| *d == name)
        .map(|index| index as u8)
        .ok_or_else(|| stage(format!("no art directory named {name}")))
}

/// Entries still free in a base art list (all of them when the install has
/// no such list).
fn free_entries(base: &BTreeMap<String, Vec<u8>>, name: &str) -> Result<usize, ConvertError> {
    let category = art_category(name)?;
    let list = match base.get(&format!("art/{name}/{name}.lst")) {
        Some(bytes) => emit::ArtList::from_base(category, bytes),
        None => emit::ArtList::new(category),
    }
    .map_err(stage)?;
    Ok(list.remaining().saturating_sub(BUILDER_RESERVE))
}

fn slicer_category(category: map_slicer::ArtCategory) -> map_builder::ArtCategory {
    match category {
        map_slicer::ArtCategory::Walls => map_builder::ArtCategory::Walls,
        map_slicer::ArtCategory::Scenery => map_builder::ArtCategory::Scenery,
        map_slicer::ArtCategory::Items => map_builder::ArtCategory::Items,
    }
}

fn slicer_role(role: map_slicer::Role) -> map_builder::Role {
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
fn map_stem(slice: &str) -> String {
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
fn cache_key(
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

/// The slice's townsfolk for the builder: the sprite sets the critter stage
/// rendered, and one placement per human reference in the window that
/// starts enabled (by its own flag and by its enable parent, the rule the
/// slicer applies to statics) and whose actor has a sprite set.
fn critter_inputs(
    inputs: &Inputs,
    cells: &[CellDoc],
    references: &[mir::Reference],
) -> Result<
    (
        Vec<map_builder::CritterArt>,
        Vec<map_builder::CritterPlacement>,
    ),
    ConvertError,
> {
    let index = crate::critters::read_index(inputs)?;
    let by_actor: BTreeMap<&str, &crate::critters::CritterEntry> =
        index.iter().map(|e| (e.actor.as_str(), e)).collect();
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
    let mut sets: BTreeMap<String, usize> = BTreeMap::new();
    let mut art = Vec::new();
    let mut placements = Vec::new();
    for reference in references {
        if reference.base_kind.as_deref() != Some("NPC_")
            || reference.flags.deleted
            || reference.flags.initially_disabled
        {
            continue;
        }
        if let Some(parent) = &reference.enable_parent {
            let parent_on = enabled
                .get(&parent.reference.to_string())
                .copied()
                .unwrap_or(true);
            if parent_on == parent.opposite {
                continue;
            }
        }
        let Some(entry) = by_actor.get(reference.base.to_string().as_str()) else {
            continue;
        };
        let Some(key) = &entry.set else {
            continue;
        };
        let name = entry
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| entry.editor_id.clone())
            .unwrap_or_else(|| "Resident".to_owned());
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
    Ok((art, placements))
}

/// Paints the map as the engine draws it: floor tiles, then every object in
/// ascending hex order, each frame bottom-centre at its hex centre plus
/// shift and displacement.
fn compose_preview(
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

/// Builds the slice's map and writes the mod layer.
pub fn map(inputs: &Inputs) -> Result<MapSummary, ConvertError> {
    let mir = Mir::open(&inputs.mir_dir()).map_err(stage)?;
    let palette_bytes = inputs.f2_file("color.pal")?;
    let palette = inputs.palette()?;
    let palette_map = PaletteMap::try_new(&palette).map_err(stage)?;

    // The base game's lists, which the layer must ship whole. A synthetic
    // install has none, and then everything starts empty.
    let mut base: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for path in map_builder::base_file_paths() {
        if let Ok(bytes) = inputs.f2_file(path) {
            base.insert((*path).to_owned(), bytes);
        }
    }

    let mod_dir = inputs.mod_dir();
    let key = cache_key(inputs, &palette_bytes, &base)?;
    let key_path = inputs.out.join("cache").join("map.key");
    let summary_path = inputs.out.join("cache").join("map.summary");
    if mod_dir.join("vault-mod.toml").is_file()
        && std::fs::read_to_string(&key_path).is_ok_and(|stored| stored == key)
        && let Ok(text) = std::fs::read_to_string(&summary_path)
        && let Some(summary) = MapSummary::from_cache(&text)
    {
        return Ok(MapSummary {
            reused: true,
            ..summary
        });
    }

    let projection = Projection::new(&inputs.calibration).map_err(stage)?;
    let window = MapWindow::from_center(inputs.slice.center(), projection.tile_units());
    let worldspace = inputs.slice.worldspace.to_ascii_lowercase();
    let sampler = GroundSampler::load(&mir, &worldspace, &window).map_err(stage)?;

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
    let references: Vec<mir::Reference> = cells
        .iter()
        .flat_map(|cell| cell.references.iter())
        .filter(|r| window.contains(r.transform.position[0], r.transform.position[1]))
        .cloned()
        .collect();

    // Interiors the window's doors open on, for the exits.
    let mut interiors: BTreeMap<String, CellDoc> = BTreeMap::new();
    for reference in &references {
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

    let budget = map_slicer::Budget {
        walls: free_entries(&base, "walls")?,
        scenery: free_entries(&base, "scenery")?,
        items: free_entries(&base, "items")?,
    };
    // Where the game's own navmesh lets an actor stand, a building's
    // bounding footprint must not block: porches, canopies, doorsteps.
    let build_options = map_builder::BuildOptions::default();
    let frame = map_builder::walk::Frame::new(&window, &projection).map_err(stage)?;
    // Only around doors that lead somewhere: opened everywhere, the strips
    // that can no longer merge overflow the walls list.
    let near_doors: BTreeSet<i32> = references
        .iter()
        .filter(|r| r.teleport.is_some())
        .filter_map(|r| {
            let [x, y, _] = r.transform.position;
            frame.hex_of_source(f64::from(x), f64::from(y))
        })
        .flat_map(|door| map_builder::exits::hexes_around(door, DOOR_APPROACH, false))
        .collect();
    let slicer_config = map_slicer::SlicerConfig {
        open_hexes: map_builder::walk::standable_hexes(&frame, &sampler, &cells, &build_options)
            .into_iter()
            .filter(|hex| near_doors.contains(hex))
            .collect(),
        ..map_slicer::SlicerConfig::default()
    };
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
        &budget,
        &slicer_config,
    )
    .map_err(stage)?;

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
    let terrain_options = map_terrain::TerrainOptions {
        library_cap: free_entries(&base, "tiles")?.min(2000),
        jobs: inputs.jobs,
        texture_lod_bias: GROUND_LOD_BIAS,
        ..map_terrain::TerrainOptions::default()
    };
    let ground = map_terrain::build_ground(
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
    .map_err(stage)?;

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
    let (critter_art, critters) = critter_inputs(inputs, &cells, &references)?;
    let mut builder_cells = cells.clone();
    builder_cells.extend(interiors.into_values());
    let stem = map_stem(&inputs.slice.name);
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
            squares: &ground.squares,
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
        "Window: {} units square, north-west corner at ({}, {}).\n",
        window.size, window.x_west, window.y_north
    );
    let _ = writeln!(text, "## Art budget\n\n```\n{}\n```\n", sliced.report);
    let _ = writeln!(text, "## Ground\n\n```\n{:#?}\n```\n", ground.stats);
    let _ = writeln!(text, "## Walkability\n\n```\n{:#?}\n```\n", report.walk);
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
            "- door {} at hex {} leads to `{}` (map {}); returns to hex {}",
            exit.door, exit.door_hex, exit.stub_file, exit.stub_number, exit.return_hex
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

    let cache_dir = inputs.out.join("cache");
    std::fs::create_dir_all(&cache_dir).map_err(io(&cache_dir))?;
    std::fs::write(&summary_path, summary.to_cache()).map_err(io(&summary_path))?;
    std::fs::write(&key_path, key).map_err(io(&key_path))?;
    Ok(summary)
}

impl MapSummary {
    fn to_cache(&self) -> String {
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

    fn from_cache(text: &str) -> Option<Self> {
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
