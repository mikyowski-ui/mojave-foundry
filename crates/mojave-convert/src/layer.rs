//! Layers: several maps in one mod layer, with interior storeys and
//! exterior levels as elevations, doors linked both ways and stairs as
//! paired exit grids (`map_builder::build_layer`, the builder's
//! `DESIGN-levels.md`).
//!
//! # The layer file
//!
//! `--slice <name>.layer.toml` names the slices built into one layer:
//!
//! ```toml
//! name = "super-duper-mart"     # the layer (report title); letters, digits, '-', '_'
//! start = "sdmart"              # the start map's stem (default: the first map)
//! with_interiors = false        # also every interior the exterior's doors lead to
//!
//! [all_maps]                    # slice keys laid over every map's slice
//! wire = true
//!
//! [[maps]]
//! slice = "super-duper-mart-exterior.toml"   # relative to this file, or a built-in name
//! map = "sdmart"                              # any slice key overrides the file's
//!
//! [[maps]]
//! slice = "super-duper-mart.toml"
//! map = "sdmartin"
//!
//! [[maps]]
//! interior = "SomeCellEditorId"  # no slice file: the first map's keys, as an interior
//! ```
//!
//! Every map's slice must be of one game, one plugin list and one
//! worldspace; extract, sprites and critters run once over their union
//! ([`LayerSpec::union_slice`]). An interior of a layer splits its storeys
//! (`storeys = "auto"` is its default there), an exterior its levels only
//! with `levels = "auto"`. `--with-interiors` on an exterior slice is a
//! layer of that slice with `with_interiors = true`.
//!
//! # The art budget
//!
//! One budget for the layer: every map and elevation is estimated
//! (`map_slicer::estimate`) and given its share of each list in proportion
//! to its demand (largest remainder); runs are sliced in map order, each
//! passing what it leaves unused to the next. Floor tiles likewise, by the
//! floor each elevation has, smallest first. A one-map, one-elevation layer
//! gets the whole budget.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use foundry_render::Projection;
use mir::{CellDoc, CellLocation, GroundSampler, MapWindow, Mir};
use quantize::PaletteMap;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::{ConvertError, Inputs, SliceSpec};
use crate::map::{
    self, Budgets, DeclutterChoice, InteriorScene, MapFeatures, MapSummary, OpenHexes, OpenPolicy,
    Person, PersonSkip, io, stage,
};

/// A layer file's name ends so.
pub const LAYER_SUFFIX: &str = ".layer.toml";

/// Bumped whenever a layer's output for the same inputs would change.
const LAYER_VERSION: &str = "layer-2";

/// Whether `name` (a `--slice` argument) names a layer file.
pub fn is_layer_file(name: &str) -> bool {
    name.ends_with(LAYER_SUFFIX)
}

/// One map of a layer: its slice (the file's keys under the layer's).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerMap {
    pub slice: SliceSpec,
    /// Added by `with_interiors`, not named in the file.
    pub automatic: bool,
}

/// A resolved layer file (module notes).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSpec {
    pub name: String,
    /// The start map's stem; `None`: the first map.
    pub start: Option<String>,
    /// Add every interior the first exterior's doors lead to.
    pub with_interiors: bool,
    /// `close_unreached = true`: what no walk from the start map's entrance
    /// reaches through the layer's links is closed and reported
    /// (`map_builder::LayerOptions::close_unreached`), except a component a
    /// person stands on. Default on.
    pub close_unreached: bool,
    pub maps: Vec<LayerMap>,
    /// The first exterior's slice as a table, and the `[all_maps]` keys:
    /// what an automatic interior is made from.
    base: Option<toml::Table>,
    all_maps: toml::Table,
    /// The file's own text (hashed into the map stage's key), empty for a
    /// layer made from one slice.
    source: String,
}

fn on() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerFile {
    name: String,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    with_interiors: bool,
    #[serde(default = "on")]
    close_unreached: bool,
    #[serde(default)]
    all_maps: toml::Table,
    #[serde(default)]
    maps: Vec<toml::Table>,
}

/// Keys of an exterior slice an interior made from it does not take.
const EXTERIOR_ONLY: &[&str] = &[
    "map_center",
    "levels",
    "navmesh_only",
    "vertex_color_floor",
    "entrance",
    "map",
    "name",
    "interior",
];

fn slice_table(slice: &SliceSpec) -> toml::Table {
    toml::from_str(&slice.to_toml()).expect("a slice spec serialises to a table")
}

fn merge(base: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

fn parse_slice(table: &toml::Table) -> Result<SliceSpec, String> {
    let text = toml::to_string(table).map_err(|e| e.to_string())?;
    SliceSpec::from_toml(&text)
}

/// A map stem from a cell's editor id: lowercase letters and digits, the
/// worldspace's name (less a trailing `world`) dropped from its head, at
/// most eight characters; a stem already taken gets two digits.
pub fn auto_stem(editor_id: &str, worldspace: &str, taken: &BTreeSet<String>) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let id = clean(editor_id);
    let world = clean(worldspace);
    let world = world.strip_suffix("world").unwrap_or(&world).to_owned();
    let body = match id.strip_prefix(&world) {
        Some(rest) if !world.is_empty() && rest.len() >= 3 => rest.to_owned(),
        _ => id.clone(),
    };
    let body = if body.is_empty() {
        "room".to_owned()
    } else {
        body
    };
    let first: String = body.chars().take(8).collect();
    if !taken.contains(&first) {
        return first;
    }
    let head: String = body.chars().take(6).collect();
    (2..100)
        .map(|n| format!("{head}{n:02}"))
        .find(|s| !taken.contains(s))
        .unwrap_or(first)
}

impl LayerSpec {
    /// Reads a layer file; slice files are found relative to it.
    pub fn load(path: &Path) -> Result<Self, ConvertError> {
        let text = std::fs::read_to_string(path).map_err(io(path))?;
        Self::from_toml(&text, path.parent().unwrap_or(Path::new("."))).map_err(|reason| {
            ConvertError::Slice {
                name: path.display().to_string(),
                reason,
            }
        })
    }

    /// Parses a layer file whose slice paths are relative to `dir`.
    pub fn from_toml(text: &str, dir: &Path) -> Result<Self, String> {
        let file: LayerFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.name.is_empty()
            || !file
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("name must be letters, digits, '-' or '_'".to_owned());
        }
        if file.maps.is_empty() && !file.with_interiors {
            return Err("a layer needs at least one [[maps]] entry".to_owned());
        }
        let mut maps = Vec::new();
        let mut base: Option<toml::Table> = None;
        for (n, entry) in file.maps.iter().enumerate() {
            let context = |e: String| format!("maps[{n}]: {e}");
            let mut over = entry.clone();
            let slice_name = over.remove("slice");
            let mut table = match slice_name {
                Some(toml::Value::String(name)) => {
                    if let Some(preset) = SliceSpec::preset(&name) {
                        slice_table(&preset)
                    } else {
                        let path: PathBuf = dir.join(&name);
                        let text = std::fs::read_to_string(&path)
                            .map_err(|e| context(format!("{}: {e}", path.display())))?;
                        toml::from_str::<toml::Table>(&text)
                            .map_err(|e| context(format!("{}: {e}", path.display())))?
                    }
                }
                Some(_) => return Err(context("slice must be a file name".to_owned())),
                None => {
                    let Some(toml::Value::String(id)) = over.get("interior").cloned() else {
                        return Err(context(
                            "give slice = \"<file>\", or interior = \"<editor id>\" after an \
                             exterior map"
                                .to_owned(),
                        ));
                    };
                    let Some(base) = &base else {
                        return Err(context(
                            "an interior without a slice file takes the keys of an exterior \
                             map listed before it"
                                .to_owned(),
                        ));
                    };
                    interior_table(base, &id)
                }
            };
            merge(&mut table, &file.all_maps);
            merge(&mut table, &over);
            if !table.contains_key("interior") && base.is_none() {
                base = Some(table.clone());
            }
            if table.contains_key("interior") && !table.contains_key("storeys") {
                table.insert("storeys".into(), toml::Value::String("auto".into()));
            }
            let slice = parse_slice(&table).map_err(context)?;
            maps.push(LayerMap {
                slice,
                automatic: false,
            });
        }
        if file.with_interiors && base.is_none() {
            return Err("with_interiors needs an exterior map".to_owned());
        }
        let spec = Self {
            name: file.name,
            start: file.start,
            with_interiors: file.with_interiors,
            close_unreached: file.close_unreached,
            maps,
            base,
            all_maps: file.all_maps,
            source: text.to_owned(),
        };
        spec.validate()?;
        Ok(spec)
    }

    /// The exterior `slice` with every interior its doors lead to.
    pub fn with_interiors_of(slice: &SliceSpec) -> Self {
        Self {
            name: slice.name.clone(),
            start: None,
            with_interiors: true,
            close_unreached: slice.close_unreached.unwrap_or(true),
            maps: vec![LayerMap {
                slice: slice.clone(),
                automatic: false,
            }],
            base: Some(slice_table(slice)),
            all_maps: toml::Table::new(),
            source: String::new(),
        }
    }

    /// One slice as a layer of one map (its storeys or levels split).
    pub fn single(slice: &SliceSpec) -> Self {
        Self {
            name: slice.name.clone(),
            start: None,
            with_interiors: false,
            close_unreached: slice.close_unreached.unwrap_or(true),
            maps: vec![LayerMap {
                slice: slice.clone(),
                automatic: false,
            }],
            base: None,
            all_maps: toml::Table::new(),
            source: String::new(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        let mut stems = BTreeSet::new();
        let first = self.maps.first().map(|m| &m.slice);
        for map in &self.maps {
            let stem = map.slice.map_stem();
            if !stems.insert(stem.clone()) {
                return Err(format!(
                    "two maps share the stem {stem:?}; give one a map = \"...\" key"
                ));
            }
            if let Some(first) = first {
                if map.slice.stated_game() != first.stated_game() {
                    return Err(format!(
                        "map {stem} is of another game than {}",
                        first.map_stem()
                    ));
                }
                if map.slice.plugins != first.plugins {
                    return Err(format!(
                        "map {stem}'s plugins differ from {}'s",
                        first.map_stem()
                    ));
                }
                if !map.slice.worldspace.eq_ignore_ascii_case(&first.worldspace) {
                    return Err(format!(
                        "map {stem} is of worldspace {}, {} of {}: one extract serves a layer",
                        map.slice.worldspace,
                        first.map_stem(),
                        first.worldspace
                    ));
                }
            }
        }
        if let Some(start) = &self.start
            && !stems.contains(start)
            && !self.with_interiors
        {
            return Err(format!("start = {start:?} names no map of the layer"));
        }
        Ok(())
    }

    /// The slice extract, sprites and critters run over: the first map's
    /// game, plugins, worldspace and critter keys; the union of every map's
    /// cells; the deepest interior depth; loot on when any map has it.
    pub fn union_slice(&self) -> SliceSpec {
        let first = &self.maps[0].slice;
        let mut x = first.x;
        let mut y = first.y;
        let mut depth = first.interior_depth.max(1);
        let mut loot = first.loot;
        for map in &self.maps[1..] {
            x = [x[0].min(map.slice.x[0]), x[1].max(map.slice.x[1])];
            y = [y[0].min(map.slice.y[0]), y[1].max(map.slice.y[1])];
            depth = depth.max(map.slice.interior_depth);
            if map.slice.loot == Some(true) {
                loot = Some(true);
            }
        }
        SliceSpec {
            name: self.name.clone(),
            game: first.stated_game(),
            worldspace: first.worldspace.clone(),
            plugins: first.plugins.clone(),
            x,
            y,
            interior_depth: depth,
            loot,
            animations: first.animations.clone(),
            deaths: first.deaths.clone(),
            stride: first.stride.clone(),
            figure_look: first.figure_look.clone(),
            ..SliceSpec::default()
        }
    }

    /// Every map, with the automatic interiors added (after extract: the
    /// first exterior's doors name them), the start map first.
    pub fn resolve(&self, inputs: &Inputs, mir: &Mir) -> Result<Vec<LayerMap>, ConvertError> {
        let mut maps = self.maps.clone();
        if self.with_interiors {
            let Some(exterior) = maps.iter().find(|m| !m.slice.is_interior()).cloned() else {
                return Err(stage("with_interiors needs an exterior map"));
            };
            let base = self
                .base
                .clone()
                .unwrap_or_else(|| slice_table(&exterior.slice));
            let listed: BTreeSet<String> = maps
                .iter()
                .filter_map(|m| m.slice.interior.as_deref())
                .map(str::to_ascii_lowercase)
                .collect();
            let mut taken: BTreeSet<String> = maps.iter().map(|m| m.slice.map_stem()).collect();
            let one = inputs.for_map(&exterior.slice);
            for id in door_interiors(&one, mir)? {
                if listed.contains(&id.to_ascii_lowercase()) {
                    continue;
                }
                let mut table = interior_table(&base, &id);
                merge(&mut table, &self.all_maps);
                let stem = auto_stem(&id, &exterior.slice.worldspace, &taken);
                taken.insert(stem.clone());
                table.insert("map".into(), toml::Value::String(stem));
                table
                    .entry("storeys")
                    .or_insert_with(|| toml::Value::String("auto".into()));
                let slice =
                    parse_slice(&table).map_err(|e| stage(format!("interior {id}: {e}")))?;
                maps.push(LayerMap {
                    slice,
                    automatic: true,
                });
            }
        }
        if let Some(start) = &self.start {
            let Some(at) = maps.iter().position(|m| &m.slice.map_stem() == start) else {
                return Err(stage(format!(
                    "start = {start:?} names no map of the layer"
                )));
            };
            let map = maps.remove(at);
            maps.insert(0, map);
        }
        Ok(maps)
    }
}

/// The table of an interior `id` made from an exterior's.
fn interior_table(base: &toml::Table, id: &str) -> toml::Table {
    let mut table = base.clone();
    for key in EXTERIOR_ONLY {
        table.remove(*key);
    }
    let name: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    table.insert("name".into(), toml::Value::String(name));
    table.insert("interior".into(), toml::Value::String(id.to_owned()));
    table.insert("place_disabled".into(), toml::Value::Boolean(true));
    table
}

/// The interiors (editor ids) the exterior slice's window doors lead to,
/// in door form-key order, each once.
fn door_interiors(inputs: &Inputs, mir: &Mir) -> Result<Vec<String>, ConvertError> {
    let projection = Projection::new(&inputs.calibration).map_err(stage)?;
    let window = MapWindow::from_center(inputs.slice.center(), projection.tile_units());
    let worldspace = inputs.slice.worldspace.to_ascii_lowercase();
    let (xs, ys) = window.cells(0);
    let mut doors: Vec<(String, mir::FormKey)> = Vec::new();
    for y in ys {
        for x in xs.clone() {
            let Some(doc) = mir.cell(&worldspace, x, y).map_err(stage)? else {
                continue;
            };
            for r in &doc.references {
                let [px, py, _] = r.transform.position;
                if !window.contains(px, py) || r.flags.deleted {
                    continue;
                }
                if let Some(cell) = r.teleport.as_ref().and_then(|t| t.cell.as_ref())
                    && matches!(cell.location, CellLocation::Interior)
                {
                    doors.push((r.form.to_string(), cell.form.clone()));
                }
            }
        }
    }
    doors.sort();
    let mut out: Vec<String> = Vec::new();
    for (_, form) in doors {
        if let Some(doc) = mir.interior_by_form(&form).map_err(stage)?
            && let Some(id) = doc.editor_id
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
    Ok(out)
}

/// Editor ids of the people a layer's maps place (its exterior windows,
/// with one cell of margin, and its interiors): what the critter stage
/// renders above the minimal level.
pub fn actor_ids(inputs: &Inputs, mir: &Mir) -> Result<Vec<String>, ConvertError> {
    let Some(layer) = inputs.layer_spec() else {
        return Ok(Vec::new());
    };
    let maps = layer.resolve(inputs, mir)?;
    let mut ids = BTreeSet::new();
    let mut take = |references: &[mir::Reference]| {
        for r in references {
            if matches!(
                r.base_kind.as_deref(),
                Some("NPC_" | "CREA" | "LVLN" | "LVLC")
            ) && let Some(id) = &r.base_editor_id
            {
                ids.insert(id.clone());
            }
        }
    };
    for map in &maps {
        if let Some(id) = &map.slice.interior {
            if let Some(doc) = mir.interior(id).map_err(stage)? {
                take(&doc.references);
            }
            continue;
        }
        let projection = Projection::new(&inputs.calibration).map_err(stage)?;
        let window = MapWindow::from_center(map.slice.center(), projection.tile_units());
        let worldspace = map.slice.worldspace.to_ascii_lowercase();
        let (xs, ys) = window.cells(1);
        for y in ys {
            for x in xs.clone() {
                if let Some(doc) = mir.cell(&worldspace, x, y).map_err(stage)? {
                    take(&doc.references);
                }
            }
        }
    }
    Ok(ids.into_iter().collect())
}

// --- Planning -------------------------------------------------------------

/// Where an elevation's ground comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Terrain {
    /// The worldspace's land (an exterior's ground level).
    Land,
    /// No land: only the slicer's baked floor, the rest blank.
    Blank(f32),
}

/// One elevation of one map: one slicer run and one terrain run.
struct Run {
    /// `"<stem> elevation <e>"` and what it is (`storey 1, floor z 174`).
    label: String,
    what: String,
    elevation: u8,
    references: Vec<mir::Reference>,
    /// The slicer's ground (anchors); `None`: the map's own sampler (an
    /// exterior's ground level). The builder's sampler is the map's.
    slicer_sampler: Option<GroundSampler>,
    slicer_config: map_slicer::SlicerConfig,
    terrain: Terrain,
    interior: Option<map_builder::InteriorBuild>,
    level: Option<map_builder::ExteriorLevel>,
    open: OpenHexes,
    /// Teleport doors whose placements stay doors here; `None`: every
    /// door. A door of another elevation is drawn as scenery.
    doors: Option<BTreeSet<String>>,
    /// Floor squares this elevation is expected to draw (tile shares).
    tile_weight: usize,
    /// References baked as this level's deck.
    decks: usize,
    // Filled as the run goes.
    estimate: Option<map_slicer::BudgetReport>,
    budget: map_slicer::Budget,
    tile_cap: usize,
    sliced: Option<map_slicer::Sliced>,
    ground: Option<map_terrain::Ground>,
    squares: Vec<u16>,
    art: Vec<map_builder::ArtImage>,
    placements: Vec<map_builder::Placement>,
    doors_demoted: usize,
}

/// One map of the layer, planned.
struct MapRun {
    inputs: Inputs,
    stem: String,
    window: MapWindow,
    projection: Projection,
    worldspace: String,
    /// The builder's sampler (the worldspace's, or the flat floor).
    sampler: GroundSampler,
    /// The scene's cells, and with the interiors the doors lead to.
    cells: Vec<CellDoc>,
    builder_cells: Vec<CellDoc>,
    references: Vec<mir::Reference>,
    build_options: map_builder::BuildOptions,
    declutter: DeclutterChoice,
    interior: Option<InteriorScene>,
    footprints_on: bool,
    /// The storey or level split, for the report.
    split_lines: Vec<String>,
    entrance_elevation: Option<u8>,
    runs: Vec<Run>,
    critters: Vec<map_builder::CritterPlacement>,
    people_skipped: Vec<PersonSkip>,
    /// For a building spilled over several maps: the z band of its
    /// storeys, to give each map only its own people.
    critter_band: Option<[f32; 2]>,
}

/// The planned layer.
struct Plan {
    maps: Vec<MapRun>,
    stairs: Vec<map_builder::StairLink>,
}

/// The open hexes of a run from its standable set.
fn open_of(
    policy: OpenPolicy,
    standable: &[i32],
    walkers: &BTreeSet<i32>,
    references: &[mir::Reference],
    frame: &map_builder::walk::Frame,
) -> (BTreeSet<i32>, OpenHexes) {
    let open: BTreeSet<i32> = match policy {
        OpenPolicy::Everywhere => standable.iter().copied().collect(),
        OpenPolicy::NearDoors => {
            let near: BTreeSet<i32> = references
                .iter()
                .filter(|r| r.teleport.is_some())
                .filter_map(|r| {
                    let [x, y, _] = r.transform.position;
                    frame.hex_of_source(f64::from(x), f64::from(y))
                })
                .flat_map(|door| map_builder::exits::hexes_around(door, map::DOOR_APPROACH, false))
                .collect();
            standable
                .iter()
                .copied()
                .filter(|h| near.contains(h))
                .collect()
        }
    };
    let counts = OpenHexes {
        policy,
        standable: standable.len(),
        opened: open.len(),
        walkers: walkers.len(),
    };
    (open, counts)
}

fn walkers_of(
    frame: &map_builder::walk::Frame,
    sampler: &GroundSampler,
    cells: &[CellDoc],
    options: &map_builder::BuildOptions,
) -> BTreeSet<i32> {
    let (verdicts, _) = map_builder::walk::verdicts(frame, sampler, cells, &[], options);
    verdicts
        .iter()
        .enumerate()
        .filter(|(_, v)| **v == map_builder::walk::Verdict::Walkable)
        .map(|(hex, _)| hex as i32)
        .collect()
}

fn empty_run(label: String, what: String, elevation: u8) -> Run {
    Run {
        label,
        what,
        elevation,
        references: Vec::new(),
        slicer_sampler: None,
        slicer_config: map_slicer::SlicerConfig::default(),
        terrain: Terrain::Land,
        interior: None,
        level: None,
        open: OpenHexes {
            policy: OpenPolicy::Everywhere,
            standable: 0,
            opened: 0,
            walkers: 0,
        },
        doors: None,
        tile_weight: 0,
        decks: 0,
        estimate: None,
        budget: map_slicer::Budget {
            walls: 0,
            scenery: 0,
            items: 0,
        },
        tile_cap: 0,
        sliced: None,
        ground: None,
        squares: Vec::new(),
        art: Vec::new(),
        placements: Vec::new(),
        doors_demoted: 0,
    }
}

/// Kinds that are never a deck.
const NOT_DECKS: &[&str] = &[
    "NPC_", "CREA", "LVLN", "LVLC", "DOOR", "CONT", "WEAP", "ARMO", "AMMO", "ALCH", "MISC", "BOOK",
    "KEYM", "NOTE", "LIGH", "SOUN", "ACTI", "TERM", "FURN", "IDLM",
];

/// Smallest plan area (units²) of a deck: about two hexes.
const DECK_MIN_AREA: f64 = 8_000.0;

/// The references that carry `level`'s walking surface: for each of its
/// navmesh triangles, the reference with the smallest placed footprint
/// whose bounds hold the triangle's centroid (plan inside, height within
/// 40 units); a reference chosen by two or more triangles is a deck.
fn deck_references(
    mir: &Mir,
    cells: &[CellDoc],
    level: &map_builder::ExteriorLevel,
    references: &[mir::Reference],
) -> BTreeSet<String> {
    let mut centroids: Vec<[f32; 3]> = Vec::new();
    for cell in cells {
        for navmesh in &cell.navmeshes {
            let form = navmesh.form.to_string();
            for (i, t) in navmesh.triangles.iter().enumerate() {
                if !level.triangles.contains(&(form.clone(), i as u32)) {
                    continue;
                }
                let mut c = [0.0f32; 3];
                let mut ok = true;
                for v in t.vertices {
                    match navmesh.vertices.get(v as usize) {
                        Some(p) => {
                            for k in 0..3 {
                                c[k] += p[k] / 3.0;
                            }
                        }
                        None => ok = false,
                    }
                }
                if ok {
                    centroids.push(c);
                }
            }
        }
    }
    struct Candidate {
        key: String,
        polygon: Vec<[f64; 2]>,
        area: f64,
        z: [f32; 2],
    }
    let candidates: Vec<Candidate> = references
        .iter()
        .filter(|r| !r.flags.deleted && r.transform.is_finite())
        .filter(|r| {
            !r.base_kind
                .as_deref()
                .is_some_and(|k| NOT_DECKS.contains(&k))
        })
        .filter_map(|r| {
            let bounds = r
                .mesh
                .as_deref()
                .and_then(|hash| mir.manifest().mesh_info.get(hash))
                .and_then(|info| info.bounds)?;
            let footprint = map_slicer::Footprint::of(&r.transform, bounds);
            (footprint.area >= DECK_MIN_AREA).then(|| Candidate {
                key: r.form.to_string(),
                polygon: footprint.polygon,
                area: footprint.area,
                z: footprint.z,
            })
        })
        .collect();
    let inside = |polygon: &[[f64; 2]], p: [f64; 2]| {
        // Convex, counter-clockwise.
        let n = polygon.len();
        n >= 3
            && (0..n).all(|i| {
                let a = polygon[i];
                let b = polygon[(i + 1) % n];
                (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= -1e-6
            })
    };
    let mut votes: BTreeMap<&str, usize> = BTreeMap::new();
    for c in &centroids {
        let p = [f64::from(c[0]), f64::from(c[1])];
        let best = candidates
            .iter()
            .filter(|k| c[2] >= k.z[0] - 40.0 && c[2] <= k.z[1] + 40.0)
            .filter(|k| inside(&k.polygon, p))
            .min_by(|a, b| a.area.total_cmp(&b.area).then_with(|| a.key.cmp(&b.key)));
        if let Some(best) = best {
            *votes.entry(best.key.as_str()).or_default() += 1;
        }
    }
    votes
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(k, _)| k.to_owned())
        .collect()
}

/// The exterior entrance target of a levels build: the slice's `entrance`,
/// else the position of the window's first door leading to another
/// worldspace (the town gate); with its height when known.
fn exterior_entrance(
    slice: &SliceSpec,
    references: &[mir::Reference],
) -> Option<([f32; 2], Option<f32>, String)> {
    if let Some(p) = slice.entrance {
        return Some((p, None, "the slice's entrance".to_owned()));
    }
    let worldspace = slice.worldspace.to_ascii_lowercase();
    references
        .iter()
        .filter(|r| !r.flags.deleted && r.transform.is_finite())
        .filter(|r| {
            r.teleport
                .as_ref()
                .and_then(|t| t.cell.as_ref())
                .is_some_and(|c| match &c.location {
                    CellLocation::Exterior { worldspace: w, .. } => {
                        !w.eq_ignore_ascii_case(&worldspace)
                    }
                    CellLocation::Interior => false,
                })
        })
        .min_by(|a, b| a.form.cmp(&b.form))
        .map(|r| {
            let [x, y, z] = r.transform.position;
            (
                [x, y],
                Some(z),
                format!("door {}, the first leading out of the worldspace", r.form),
            )
        })
}

/// Plans one exterior map: one run, or one per level with `levels = "auto"`.
fn plan_exterior(
    inputs: &Inputs,
    mir: &Mir,
    map_index: usize,
    stairs: &mut Vec<map_builder::StairLink>,
) -> Result<MapRun, ConvertError> {
    let mut scene = map::scene(inputs, mir)?;
    let declutter = map::apply_declutter(inputs, mir, &mut scene)?;
    let footprints_on = inputs.slice.footprints == Some(true);
    if footprints_on {
        scene.slicer_config.furniture = Some(map_slicer::FurnitureRule::default());
    }
    map::apply_slice_keys(&inputs.slice, &mut scene);
    let frame = map_builder::walk::Frame::new(&scene.window, &scene.projection).map_err(stage)?;
    let mut runs = Vec::new();
    let mut split_lines = Vec::new();
    let mut entrance_elevation = None;
    let stem = inputs.slice.map_stem();
    let levels = inputs.slice.levels.as_deref() == Some("auto");
    let split = levels.then(|| {
        map_builder::levels::split_levels(
            &scene.cells,
            &frame,
            &scene.sampler,
            &map_builder::levels::LevelOptions::default(),
        )
    });
    if let Some(split) = split.as_ref().filter(|s| s.level_count() > 1) {
        split_lines = split.summary();
        let entrance = exterior_entrance(&inputs.slice, &scene.references);
        if let Some((point, z, from)) = &entrance {
            scene.build_options.entrance_near = Some(*point);
            let level = match z {
                Some(z) => split.level_of_point([point[0], point[1], *z]),
                None => (0..split.level_count())
                    .find(|k| split.surface_height(*k, point[0], point[1]).is_some()),
            };
            entrance_elevation = Some(level.unwrap_or(0));
            split_lines.push(format!(
                "entrance sought near ({:.0}, {:.0}), from {from}, on level {}",
                point[0],
                point[1],
                level.unwrap_or(0)
            ));
        }
        let policy = OpenPolicy::of(inputs.standalone);
        let door_level = |r: &mir::Reference| {
            let [x, y, z] = r.transform.position;
            split.level_of_point([x, y, z]).unwrap_or(0)
        };
        for k in 0..split.level_count() {
            let level = split.exterior_level(k);
            let options = map_builder::BuildOptions {
                level: Some(level.clone()),
                ..scene.build_options.clone()
            };
            let standable =
                map_builder::walk::standable_hexes(&frame, &scene.sampler, &scene.cells, &options);
            let walkers = walkers_of(&frame, &scene.sampler, &scene.cells, &options);
            let decks = if k == 0 {
                BTreeSet::new()
            } else {
                deck_references(mir, &scene.cells, &level, &scene.references)
            };
            // Level 0 draws the whole town (the decks above stand over the
            // crater floor as they did on one level); a level above draws
            // its own references, anchored on themselves, and its decks
            // baked flat as its floor.
            let references: Vec<mir::Reference> = if k == 0 {
                scene.references.clone()
            } else {
                scene
                    .references
                    .iter()
                    .filter(|r| {
                        let key = r.form.to_string();
                        if decks.contains(&key) {
                            return true;
                        }
                        let [x, y, z] = r.transform.position;
                        if r.teleport.is_some() {
                            return door_level(r) == k;
                        }
                        split.reference_level([x, y, z]) == Some(k)
                    })
                    .cloned()
                    .collect()
            };
            let doors: BTreeSet<String> = scene
                .references
                .iter()
                .filter(|r| r.teleport.is_some() && door_level(r) == k)
                .map(|r| r.form.to_string())
                .collect();
            let (open, counts) = open_of(policy, &standable, &walkers, &references, &frame);
            let config = map_slicer::SlicerConfig {
                open_hexes: open,
                landmark_walkers: Some(walkers),
                flat_references: decks.clone(),
                ..scene.slicer_config.clone()
            };
            let summary = split.levels.iter().find(|l| l.level == k);
            let mean_z = summary.map_or(0.0, |s| (s.z_range[0] + s.z_range[1]) * 0.5);
            let mut run = empty_run(
                format!("{stem} elevation {k}"),
                format!(
                    "level {k}: {} navmesh hexes, z {:.0} to {:.0}, {} references, {} decks baked flat",
                    summary.map_or(0, |s| s.hexes),
                    summary.map_or(0.0, |s| s.z_range[0]),
                    summary.map_or(0.0, |s| s.z_range[1]),
                    references.len(),
                    decks.len()
                ),
                k,
            );
            run.tile_weight = if k == 0 {
                10_000
            } else {
                (counts.standable / 2).max(16)
            };
            run.references = references;
            run.slicer_sampler = (k > 0).then(GroundSampler::default);
            run.slicer_config = config;
            run.terrain = if k == 0 {
                Terrain::Land
            } else {
                Terrain::Blank(mean_z)
            };
            run.level = Some(level);
            run.open = counts;
            run.doors = Some(doors);
            run.decks = decks.len();
            runs.push(run);
        }
        stairs.extend(split.stair_links(map_index));
    } else {
        if let Some(split) = &split {
            split_lines = split.summary();
            split_lines.push("one level: built as before".to_owned());
        }
        let mut run = empty_run(
            format!("{stem} elevation 0"),
            "the exterior window".to_owned(),
            0,
        );
        run.tile_weight = 10_000;
        run.references = scene.references.clone();
        run.slicer_config = scene.slicer_config.clone();
        run.open = scene.open;
        runs.push(run);
    }
    // Interiors the window's doors open on, for return positions.
    let mut builder_cells = scene.cells.clone();
    let mut seen = BTreeSet::new();
    for r in &scene.references {
        if let Some(cell) = r.teleport.as_ref().and_then(|t| t.cell.as_ref())
            && matches!(cell.location, CellLocation::Interior)
            && seen.insert(cell.form.to_string())
            && let Some(doc) = mir.interior_by_form(&cell.form).map_err(stage)?
        {
            builder_cells.push(doc);
        }
    }
    let mut options = scene.build_options.clone();
    options.level = None;
    Ok(MapRun {
        inputs: inputs.clone(),
        stem,
        window: scene.window,
        projection: scene.projection,
        worldspace: scene.worldspace,
        sampler: scene.sampler,
        cells: scene.cells,
        builder_cells,
        references: scene.references,
        build_options: options,
        declutter,
        interior: None,
        footprints_on,
        split_lines,
        entrance_elevation,
        runs,
        critters: Vec::new(),
        people_skipped: Vec::new(),
        critter_band: None,
    })
}

/// Plans one interior: one run per storey with `storeys = "auto"` (the
/// three largest, or `storey_floors`), else today's one storey.
fn plan_interior(
    inputs: &Inputs,
    mir: &Mir,
    first_map: usize,
    stairs: &mut Vec<map_builder::StairLink>,
) -> Result<Vec<MapRun>, ConvertError> {
    let slice = &inputs.slice;
    let id = slice.interior.clone().unwrap_or_default();
    let mut scene = map::interior_scene(inputs, mir, &id)?;
    let declutter = map::apply_declutter(inputs, mir, &mut scene)?;
    let footprints_on = slice.footprints == Some(true);
    if footprints_on {
        scene.slicer_config.furniture = Some(map_slicer::FurnitureRule::default());
    }
    map::apply_slice_keys(slice, &mut scene);
    let stem = slice.map_stem();
    let frame = map_builder::walk::Frame::new(&scene.window, &scene.projection).map_err(stage)?;
    let base_build = scene
        .build_options
        .interior
        .clone()
        .expect("an interior scene builds an interior");
    let mut options = scene.build_options.clone();
    options.interior = None;
    let builder_cells = scene.cells.clone();
    let room_floor = scene.interior.as_ref().map_or(0.0, |i| i.floor_z);
    let make = |stem: String,
                runs: Vec<Run>,
                lines: Vec<String>,
                entrance: Option<u8>,
                band: Option<[f32; 2]>| MapRun {
        inputs: inputs.clone(),
        stem,
        window: scene.window,
        projection: scene.projection.clone(),
        worldspace: scene.worldspace.clone(),
        sampler: GroundSampler::flat(room_floor),
        cells: scene.cells.clone(),
        builder_cells: builder_cells.clone(),
        references: scene.references.clone(),
        build_options: options.clone(),
        declutter: declutter.clone(),
        interior: scene.interior.clone(),
        footprints_on,
        split_lines: lines,
        entrance_elevation: entrance,
        runs,
        critters: Vec::new(),
        people_skipped: Vec::new(),
        critter_band: band,
    };

    let auto = slice.storeys.as_deref() == Some("auto");
    let split_options = map_builder::storeys::SplitOptions {
        below: base_build.below,
        above: base_build.above,
        ..map_builder::storeys::SplitOptions::default()
    };
    let split = auto.then(|| map_builder::storeys::split_storeys(&scene.cells, &split_options));
    // Storeys: the split's floors grouped by height (a floor whose height
    // lies in a larger floor's band is that storey's: a raised dais, an
    // area behind a counter the navmesh leaves unjoined), lowest first.
    let storeys: Vec<Storey> = split
        .as_ref()
        .map(|split| group_floors(split, &split_options))
        .unwrap_or_default();
    let chosen: Vec<usize> = match &slice.storey_floors {
        Some(chosen) => {
            if let Some(bad) = chosen.iter().find(|f| **f >= storeys.len()) {
                return Err(ConvertError::Slice {
                    name: slice.name.clone(),
                    reason: format!(
                        "storey_floors names storey {bad}; the cell has {} (0 to {})",
                        storeys.len(),
                        storeys.len().saturating_sub(1)
                    ),
                });
            }
            chosen.clone()
        }
        None => {
            // At most three: the largest, in height order.
            let mut by_area: Vec<usize> = (0..storeys.len()).collect();
            by_area.sort_by(|a, b| storeys[*b].area.total_cmp(&storeys[*a].area).then(a.cmp(b)));
            let mut keep: Vec<usize> = by_area.into_iter().take(3).collect();
            keep.sort_unstable();
            keep
        }
    };
    let mut lines = Vec::new();
    if let Some(split) = &split {
        lines.push(format!(
            "storeys split: {} navmesh floors in {} storeys, {} connectors ({} of {} triangles), \
             triangles {} by shared edges",
            split.floors.len(),
            storeys.len(),
            split.connectors.len(),
            split.connector_triangles,
            split.triangles,
            if split.neighbours {
                "linked"
            } else {
                "not linked (z bands)"
            }
        ));
        for (n, storey) in storeys.iter().enumerate() {
            let floors: Vec<String> = storey
                .floors
                .iter()
                .map(|f| {
                    let f = &split.floors[*f];
                    format!("z {:.1} ({:.0} units²)", f.floor_z, f.area)
                })
                .collect();
            lines.push(format!(
                "storey {n}: floor z {:.1}, {:.0} square units, navmesh floors {}{}",
                storey.floor_z,
                storey.area,
                floors.join(", "),
                match chosen.iter().position(|c| *c == n) {
                    Some(e) => format!("; elevation {e}"),
                    None => "; left out".to_owned(),
                }
            ));
        }
        for c in &split.connectors {
            let landings: Vec<String> = c
                .landings
                .iter()
                .map(|l| {
                    format!(
                        "storey {} at ({:.0}, {:.0}, {:.0})",
                        storey_of(&storeys, l.floor).map_or("-".to_owned(), |s| s.to_string()),
                        l.point[0],
                        l.point[1],
                        l.point[2]
                    )
                })
                .collect();
            lines.push(format!(
                "connector: {} triangles, z {:.0} to {:.0}, landings {}",
                c.triangles,
                c.z_range[0],
                c.z_range[1],
                if landings.is_empty() {
                    "none".to_owned()
                } else {
                    landings.join(", ")
                }
            ));
        }
    }
    if chosen.len() < 2 {
        // One storey: today's run, at the chosen storey when one is named.
        let mut build = base_build.clone();
        let mut config = scene.slicer_config.clone();
        if let [n] = chosen.as_slice() {
            let z = storeys[*n].floor_z;
            let mut one = slice.interior_build(z);
            one.entrance_near = build.entrance_near;
            build = one;
            config.interior = Some(slice.interior_slice(z));
            lines.push(format!("one storey built: storey {n}, z {z:.1}"));
        } else if split.is_some() {
            lines.push("one storey: built as before".to_owned());
        }
        let mut run = empty_run(
            format!("{stem} elevation 0"),
            format!("floor z {:.1}", build.floor_z),
            0,
        );
        run.tile_weight = (scene.open.standable / 2).max(16);
        run.references = scene.references.clone();
        let floor_sampler = GroundSampler::flat(build.floor_z);
        run.terrain = Terrain::Blank(build.floor_z);
        if chosen.len() == 1 {
            // The open hexes of the chosen storey.
            let opts = map_builder::BuildOptions {
                interior: Some(build.clone()),
                ..options.clone()
            };
            let standable =
                map_builder::walk::standable_hexes(&frame, &floor_sampler, &scene.cells, &opts);
            let walkers = walkers_of(&frame, &floor_sampler, &scene.cells, &opts);
            config.open_hexes = standable.iter().copied().collect();
            run.open = OpenHexes {
                policy: OpenPolicy::Everywhere,
                standable: standable.len(),
                opened: standable.len(),
                walkers: walkers.len(),
            };
            config.landmark_walkers = Some(walkers);
        } else {
            run.open = scene.open;
        }
        run.slicer_sampler = Some(floor_sampler);
        run.slicer_config = config;
        run.interior = Some(build);
        return Ok(vec![make(stem, vec![run], lines, None, None)]);
    }
    let split = split.expect("storeys come from a split");
    // The entrance storey: the one nearest the entrance door's height.
    let nearest = |z: f32| {
        chosen
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (storeys[*a.1].floor_z - z)
                    .abs()
                    .total_cmp(&(storeys[*b.1].floor_z - z).abs())
            })
            .map(|(e, _)| e)
    };
    let entrance_z = map::first_door(&scene.references).map(|d| d.transform.position[2]);
    let entrance_elevation = entrance_z.and_then(nearest).unwrap_or(0);
    let door_elevation: BTreeMap<String, usize> = scene
        .references
        .iter()
        .filter(|r| r.teleport.is_some())
        .filter_map(|r| nearest(r.transform.position[2]).map(|e| (r.form.to_string(), e)))
        .collect();
    let mut runs = Vec::new();
    for (e, n) in chosen.iter().enumerate() {
        let storey = &storeys[*n];
        let z = storey.floor_z;
        let mut build = slice.interior_build(z);
        if e == entrance_elevation {
            build.entrance_near = base_build.entrance_near;
        }
        // The storey walks its own floors' triangles, at any height: a
        // floor that dips at a stair head (the Super-Duper Mart's upper
        // level) is not cut by the band into islands.
        build.triangles = Some(std::sync::Arc::new(
            storey
                .floors
                .iter()
                .flat_map(|f| split.floors[*f].members.iter().cloned())
                .collect(),
        ));
        let mut cut = slice.interior_slice(z);
        // Never into the storey above.
        if let Some(above) = storeys
            .iter()
            .map(|g| g.floor_z)
            .filter(|g| *g > z + build.above * 0.5)
            .min_by(f32::total_cmp)
        {
            cut.cut_height = cut.cut_height.min((above - z - 10.0).max(40.0));
            if cut.floor_band[1] >= cut.cut_height {
                cut.floor_band[1] = cut.cut_height - 1.0;
            }
        }
        let opts = map_builder::BuildOptions {
            interior: Some(build.clone()),
            ..options.clone()
        };
        let sampler = GroundSampler::flat(z);
        let standable = map_builder::walk::standable_hexes(&frame, &sampler, &scene.cells, &opts);
        let walkers = walkers_of(&frame, &sampler, &scene.cells, &opts);
        let doors: BTreeSet<String> = door_elevation
            .iter()
            .filter(|(_, g)| **g == e)
            .map(|(k, _)| k.clone())
            .collect();
        let mut config = scene.slicer_config.clone();
        config.interior = Some(cut.clone());
        config.open_hexes = standable.iter().copied().collect();
        config.landmark_walkers = Some(walkers.clone());
        let mut run = empty_run(
            format!("{stem} elevation {e}"),
            format!(
                "storey {n}: floor z {z:.1}, cut {:.0} above it, {:.0} square units",
                cut.cut_height, storey.area
            ),
            e as u8,
        );
        run.tile_weight = (standable.len() / 2).max(16);
        run.references = scene.references.clone();
        run.slicer_sampler = Some(sampler);
        run.slicer_config = config;
        run.terrain = Terrain::Blank(z);
        run.interior = Some(build);
        run.open = OpenHexes {
            policy: OpenPolicy::Everywhere,
            standable: standable.len(),
            opened: standable.len(),
            walkers: walkers.len(),
        };
        run.doors = Some(doors);
        runs.push(run);
    }
    // Stairs: each connector's landings on two kept storeys.
    let mut seen: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut count = 0;
    for connector in &split.connectors {
        let mut ends: Vec<(usize, [f32; 3])> = connector
            .landings
            .iter()
            .filter_map(|l| {
                let s = storey_of(&storeys, l.floor)?;
                let e = chosen.iter().position(|c| *c == s)?;
                Some((e, l.point))
            })
            .collect();
        ends.sort_by_key(|(e, _)| *e);
        ends.dedup_by_key(|(e, _)| *e);
        for pair in ends.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let n = seen.entry((a.0, b.0)).or_default();
            *n += 1;
            let label = if *n == 1 {
                format!("{stem} stairs {}-{}", a.0, b.0)
            } else {
                format!("{stem} stairs {}-{} #{n}", a.0, b.0)
            };
            stairs.push(map_builder::StairLink {
                label,
                ends: [
                    map_builder::Landing {
                        map: first_map,
                        elevation: a.0 as u8,
                        point: a.1,
                    },
                    map_builder::Landing {
                        map: first_map,
                        elevation: b.0 as u8,
                        point: b.1,
                    },
                ],
            });
            count += 1;
        }
    }
    lines.push(format!(
        "{} storeys as elevations 0 to {} (entrance on {entrance_elevation}); {count} stairs between them",
        chosen.len(),
        chosen.len() - 1
    ));
    Ok(vec![make(
        stem,
        runs,
        lines,
        Some(entrance_elevation as u8),
        None,
    )])
}

/// Floors of one height: a storey.
#[derive(Debug, Clone, PartialEq)]
struct Storey {
    /// The largest floor's height, and the floors (split indices).
    floor_z: f32,
    area: f32,
    floors: Vec<usize>,
}

/// Groups the split's floors into storeys: largest first, a floor joins
/// the first storey whose band (`below`, `above` around its height) holds
/// its height; storeys sorted by height.
fn group_floors(
    split: &map_builder::storeys::StoreySplit,
    options: &map_builder::storeys::SplitOptions,
) -> Vec<Storey> {
    let mut order: Vec<usize> = (0..split.floors.len()).collect();
    order.sort_by(|a, b| {
        split.floors[*b]
            .area
            .total_cmp(&split.floors[*a].area)
            .then(a.cmp(b))
    });
    let mut storeys: Vec<Storey> = Vec::new();
    for f in order {
        let z = split.floors[f].floor_z;
        match storeys
            .iter_mut()
            .find(|s| z >= s.floor_z - options.below && z <= s.floor_z + options.above)
        {
            Some(s) => {
                s.floors.push(f);
                s.area += split.floors[f].area;
            }
            None => storeys.push(Storey {
                floor_z: z,
                area: split.floors[f].area,
                floors: vec![f],
            }),
        }
    }
    for s in &mut storeys {
        s.floors.sort_unstable();
    }
    storeys.sort_by(|a, b| a.floor_z.total_cmp(&b.floor_z));
    storeys
}

fn storey_of(storeys: &[Storey], floor: usize) -> Option<usize> {
    storeys.iter().position(|s| s.floors.contains(&floor))
}

/// Plans every map of the layer, in order.
fn plan(inputs: &Inputs, mir: &Mir, maps: &[LayerMap]) -> Result<Plan, ConvertError> {
    let mut plan = Plan {
        maps: Vec::new(),
        stairs: Vec::new(),
    };
    for map in maps {
        let one = inputs.for_map(&map.slice);
        let index = plan.maps.len();
        if map.slice.is_interior() {
            let runs = plan_interior(&one, mir, index, &mut plan.stairs)?;
            plan.maps.extend(runs);
        } else {
            let run = plan_exterior(&one, mir, index, &mut plan.stairs)?;
            plan.maps.push(run);
        }
    }
    Ok(plan)
}

// --- The budget split -----------------------------------------------------

/// Shares of `total` in proportion to `weights` by the largest remainder,
/// ties to the earlier; all-zero weights share equally.
pub fn shares(total: usize, weights: &[usize]) -> Vec<usize> {
    if weights.is_empty() {
        return Vec::new();
    }
    let sum: u128 = weights.iter().map(|w| *w as u128).sum();
    if sum == 0 {
        let n = weights.len();
        return (0..n)
            .map(|i| total / n + usize::from(i < total % n))
            .collect();
    }
    let mut out: Vec<usize> = Vec::with_capacity(weights.len());
    let mut rest: Vec<(u128, usize)> = Vec::with_capacity(weights.len());
    for (i, w) in weights.iter().enumerate() {
        let exact = total as u128 * *w as u128;
        out.push((exact / sum) as usize);
        rest.push((exact % sum, i));
    }
    let given: usize = out.iter().sum();
    rest.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, i) in rest.into_iter().take(total - given) {
        out[i] += 1;
    }
    out
}

fn category_use(report: &map_slicer::BudgetReport, category: map_slicer::ArtCategory) -> usize {
    report
        .categories
        .iter()
        .find(|c| c.category == category)
        .map_or(0, |c| c.used)
}

// --- The stage ------------------------------------------------------------

/// The layer's key: the stage's own key for every map's inputs, the
/// layer's file, its stems and order.
fn layer_key(
    inputs: &Inputs,
    layer: &LayerSpec,
    maps: &[LayerMap],
    palette_bytes: &[u8],
    base: &BTreeMap<String, Vec<u8>>,
) -> Result<(String, Vec<(String, String)>), ConvertError> {
    let mut hasher = Sha256::new();
    hasher.update(LAYER_VERSION.as_bytes());
    hasher.update(layer.source.as_bytes());
    hasher.update(format!("start={:?} interiors={}", layer.start, layer.with_interiors).as_bytes());
    if inputs.standalone {
        hasher.update(b"standalone");
    }
    let mut per_map = Vec::new();
    for map in maps {
        let one = inputs.for_map(&map.slice);
        let key = map::cache_key(&one, palette_bytes, base)?;
        hasher.update(map.slice.map_stem().as_bytes());
        hasher.update(key.as_bytes());
        per_map.push((map.slice.map_stem(), key));
    }
    let key = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((key, per_map))
}

/// Prints the layer's estimate: every map and elevation against its share.
pub fn estimate_layer(inputs: &Inputs, layer: &LayerSpec) -> Result<String, ConvertError> {
    let mir = Mir::open(&inputs.mir_dir()).map_err(stage)?;
    let base = map::base_files(inputs);
    let budgets = map::budgets(&base, inputs.standalone)?;
    let maps = layer.resolve(inputs, &mir)?;
    let mut plan = plan(inputs, &mir, &maps)?;
    let assets = map_slicer::MirAssets::new(&mir, 0);
    estimate_runs(&assets, &mut plan, &budgets)?;
    Ok(budget_text(&plan, &budgets))
}

/// Estimates every run and gives each its share of the slicer budget.
fn estimate_runs(
    assets: &dyn map_slicer::Assets,
    plan: &mut Plan,
    budgets: &Budgets,
) -> Result<(), ConvertError> {
    let mut demand: Vec<[usize; 3]> = Vec::new();
    for map in &mut plan.maps {
        for run in &mut map.runs {
            let report = map_slicer::estimate_with(
                assets,
                &run.references,
                &map.window,
                run.slicer_sampler.as_ref().unwrap_or(&map.sampler),
                &map.projection,
                &budgets.slicer,
                &run.slicer_config,
            )
            .map_err(|e| stage(format!("{}: {e}", run.label)))?;
            demand.push([
                category_use(&report, map_slicer::ArtCategory::Walls),
                category_use(&report, map_slicer::ArtCategory::Scenery),
                category_use(&report, map_slicer::ArtCategory::Items),
            ]);
            run.estimate = Some(report);
        }
    }
    let column = |c: usize| demand.iter().map(|d| d[c]).collect::<Vec<usize>>();
    let walls = shares(budgets.slicer.walls, &column(0));
    let scenery = shares(budgets.slicer.scenery, &column(1));
    let items = shares(budgets.slicer.items, &column(2));
    let weights: Vec<usize> = plan
        .maps
        .iter()
        .flat_map(|m| m.runs.iter().map(|r| r.tile_weight))
        .collect();
    let tiles = shares(budgets.tile_cap, &weights);
    let mut i = 0;
    for map in &mut plan.maps {
        for run in &mut map.runs {
            run.budget = map_slicer::Budget {
                walls: walls[i],
                scenery: scenery[i],
                items: items[i],
            };
            run.tile_cap = tiles[i];
            i += 1;
        }
    }
    Ok(())
}

fn budget_text(plan: &Plan, budgets: &Budgets) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "| map, elevation | references | demand walls / scenery / items (estimate) | budget walls / scenery / items | tile budget | used walls / scenery / items | tiles |\n|---|---:|---|---|---:|---|---:|"
    );
    let mut used_total = [0usize; 4];
    for map in &plan.maps {
        for run in &map.runs {
            let d = run.estimate.as_ref().map_or([0; 3], |e| {
                [
                    category_use(e, map_slicer::ArtCategory::Walls),
                    category_use(e, map_slicer::ArtCategory::Scenery),
                    category_use(e, map_slicer::ArtCategory::Items),
                ]
            });
            let used = run.sliced.as_ref().map(|s| {
                [
                    category_use(&s.report, map_slicer::ArtCategory::Walls),
                    category_use(&s.report, map_slicer::ArtCategory::Scenery),
                    category_use(&s.report, map_slicer::ArtCategory::Items),
                ]
            });
            let tiles = run.ground.as_ref().map(|g| g.tiles.len());
            if let Some(u) = used {
                for k in 0..3 {
                    used_total[k] += u[k];
                }
            }
            used_total[3] += tiles.unwrap_or(0);
            let _ = writeln!(
                text,
                "| {} ({}) | {} | {} / {} / {} | {} / {} / {} | {} | {} | {} |",
                run.label,
                run.what,
                run.references.len(),
                d[0],
                d[1],
                d[2],
                run.budget.walls,
                run.budget.scenery,
                run.budget.items,
                run.tile_cap,
                used.map_or("-".to_owned(), |u| format!(
                    "{} / {} / {}",
                    u[0], u[1], u[2]
                )),
                tiles.map_or("-".to_owned(), |t| t.to_string())
            );
        }
    }
    let _ = writeln!(
        text,
        "| total | | | {} / {} / {} | {} | {} / {} / {} | {} |",
        budgets.slicer.walls,
        budgets.slicer.scenery,
        budgets.slicer.items,
        budgets.tile_cap,
        used_total[0],
        used_total[1],
        used_total[2],
        used_total[3]
    );
    text
}

/// Points every teleport at the cell its partner door was extracted into.
///
/// A door outside often lives in the worldspace's persistent cell, so the
/// teleport of the door inside names that cell, which no map is built
/// from, while the extract filed the door under the grid cell it stands
/// in. A teleport whose cell no map of the layer is built from, and whose
/// partner door is a reference of a map's cells, is pointed at the cell
/// holding the partner. Returns how many were changed.
fn normalise_teleports(plan: &mut Plan) -> usize {
    let built_from: BTreeSet<String> = plan
        .maps
        .iter()
        .flat_map(|m| m.cells.iter().map(|c| c.form.to_string()))
        .collect();
    let mut holder: BTreeMap<String, mir::CellRef> = BTreeMap::new();
    for map in &plan.maps {
        for cell in &map.cells {
            for r in &cell.references {
                holder
                    .entry(r.form.to_string())
                    .or_insert_with(|| mir::CellRef {
                        form: cell.form.clone(),
                        editor_id: cell.editor_id.clone(),
                        location: cell.location.clone(),
                    });
            }
        }
    }
    let mut changed = 0;
    for map in &mut plan.maps {
        for cell in &mut map.builder_cells {
            for r in &mut cell.references {
                let Some(teleport) = &mut r.teleport else {
                    continue;
                };
                let unresolved = teleport
                    .cell
                    .as_ref()
                    .is_none_or(|c| !built_from.contains(&c.form.to_string()));
                if unresolved && let Some(partner) = holder.get(&teleport.door.to_string()) {
                    teleport.cell = Some(partner.clone());
                    changed += 1;
                }
            }
        }
    }
    changed
}

/// Drops the placements of teleport doors that belong to another
/// elevation (a door is converted on its own elevation only).
fn demote_doors(run: &mut Run) {
    let Some(doors) = &run.doors else {
        return;
    };
    let teleports: BTreeSet<String> = run
        .references
        .iter()
        .filter(|r| r.teleport.is_some())
        .map(|r| r.form.to_string())
        .collect();
    let before = run.placements.len();
    run.placements
        .retain(|p| !(teleports.contains(&p.reference) && !doors.contains(&p.reference)));
    run.doors_demoted = before - run.placements.len();
}

/// Builds the layer's maps and writes the layer (module notes).
pub fn map_layer(inputs: &Inputs, layer: &LayerSpec) -> Result<MapSummary, ConvertError> {
    let started = std::time::Instant::now();
    let mir = Mir::open(&inputs.mir_dir()).map_err(stage)?;
    let palette_bytes = inputs.f2_file("color.pal")?;
    let palette = inputs.palette()?;
    let palette_map = PaletteMap::try_new(&palette).map_err(stage)?;
    let base = map::base_files(inputs);
    let maps = layer.resolve(inputs, &mir)?;

    let mod_dir = inputs.mod_dir();
    let (key, per_map_keys) = layer_key(inputs, layer, &maps, &palette_bytes, &base)?;
    let cache_dir = inputs.out.join("cache");
    let key_path = cache_dir.join("map.key");
    let summary_path = cache_dir.join("map.summary");
    let features_path = inputs.out.join(map::FEATURES_FILE);
    if mod_dir.join("vault-mod.toml").is_file()
        && features_path.is_file()
        && crate::placements::is_current(&inputs.out)
        && std::fs::read_to_string(&key_path).is_ok_and(|stored| stored == key)
        && let Ok(text) = std::fs::read_to_string(&summary_path)
        && let Some(summary) = MapSummary::from_cache(&text)
    {
        return Ok(MapSummary {
            reused: true,
            ..summary
        });
    }

    let budgets = map::budgets(&base, inputs.standalone)?;
    // Every mesh is read and every texture decoded once for the estimate
    // and all the slicer runs (`crate::cache::SharedAssets`).
    let assets = crate::cache::SharedAssets::new(
        &mir,
        map_slicer::SlicerConfig::default().texture_cache_bytes,
    );
    let plan_time = crate::timings::span("plan the layer");
    let mut plan = plan(inputs, &mir, &maps)?;
    let redirected = normalise_teleports(&mut plan);
    drop(plan_time);
    let estimate_time = crate::timings::span("estimate");
    estimate_runs(&assets, &mut plan, &budgets)?;
    drop(estimate_time);

    // Slice every run in order, passing on what each leaves unused.
    let mut renderer =
        foundry_render::renderer(inputs.backend, &inputs.calibration).map_err(stage)?;
    // Renders and grounds come from the content-hash store when their
    // inputs are unchanged (`crate::cache`).
    let store = crate::cache::Store::open(&inputs.store_dir());
    let calibration_toml = inputs.calibration.to_toml().map_err(stage)?;
    let texture_id = |path: &str| mir.texture_hash(path).map(str::to_owned);
    let manifest_digest = map::manifest_digest(inputs)?;
    // Interiors first, then the levels above an exterior's ground, its
    // ground level last: estimates of a room overcount what its cut
    // leaves, so what the rooms leave over goes to the ground level, whose
    // budget ladder works best.
    let mut slice_order: Vec<(u8, usize, usize)> = Vec::new();
    for (m, map) in plan.maps.iter().enumerate() {
        for (r, run) in map.runs.iter().enumerate() {
            let rank = match (map.interior.is_some(), run.elevation) {
                (true, _) => 0,
                (false, e) if e > 0 => 1,
                _ => 2,
            };
            slice_order.push((rank, m, r));
        }
    }
    slice_order.sort();
    let mut carry = [0usize; 3];
    for (_, m, r) in slice_order {
        let map = &mut plan.maps[m];
        {
            let run = &mut map.runs[r];
            run.budget.walls += carry[0];
            run.budget.scenery += carry[1];
            run.budget.items += carry[2];
            let _t = crate::timings::span(format!("slice {}", run.label));
            let mut cached = crate::cache::CachedRenderer::new(
                renderer.as_mut(),
                &calibration_toml,
                &store,
                &texture_id,
            );
            let sliced = map_slicer::slice_with(
                &assets,
                &run.references,
                &map.window,
                run.slicer_sampler.as_ref().unwrap_or(&map.sampler),
                &map.projection,
                &mut cached,
                &palette_map,
                &run.budget,
                &run.slicer_config,
            )
            .map_err(|e| stage(format!("{}: {e}", run.label)))?;
            cached.note("renderer");
            let used = [
                category_use(&sliced.report, map_slicer::ArtCategory::Walls),
                category_use(&sliced.report, map_slicer::ArtCategory::Scenery),
                category_use(&sliced.report, map_slicer::ArtCategory::Items),
            ];
            carry = [
                run.budget.walls.saturating_sub(used[0]),
                run.budget.scenery.saturating_sub(used[1]),
                run.budget.items.saturating_sub(used[2]),
            ];
            run.art = sliced
                .art
                .iter()
                .map(|a| map_builder::ArtImage {
                    category: map::slicer_category(a.category),
                    width: a.width,
                    height: a.height,
                    pixels: a.pixels.clone(),
                    shift: a.shift,
                    source: a.source.clone(),
                })
                .collect();
            run.placements = sliced
                .placements
                .iter()
                .map(|p| map_builder::Placement {
                    art: p.art,
                    hex: p.hex,
                    displacement: p.displacement,
                    role: map::slicer_role(p.role),
                    reference: p.reference.clone(),
                    blocking: p.blocking,
                })
                .collect();
            demote_doors(run);
            run.sliced = Some(sliced);
        }
    }

    // Ground: smallest tile demand first, each passing on what it leaves.
    let mut order: Vec<(usize, usize, usize)> = Vec::new();
    for (m, map) in plan.maps.iter().enumerate() {
        for (r, run) in map.runs.iter().enumerate() {
            order.push((run.tile_weight, m, r));
        }
    }
    order.sort();
    let mut tile_carry = 0usize;
    for (_, m, r) in order {
        let map = &mut plan.maps[m];
        let run = &mut map.runs[r];
        run.tile_cap += tile_carry;
        let sliced = run.sliced.as_ref().expect("sliced above");
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
        let blank = matches!(run.terrain, Terrain::Blank(_));
        let _t = crate::timings::span(format!("terrain {}", run.label));
        let options = map_terrain::TerrainOptions {
            library_cap: run.tile_cap.max(1),
            library_method: if inputs.standalone {
                map_terrain::LibraryMethod::GreedyMerge
            } else {
                map_terrain::TerrainOptions::default().library_method
            },
            jobs: inputs.jobs,
            texture_lod_bias: map::GROUND_LOD_BIAS,
            blank_without_land: blank,
            tone: map::terrain_tone(&map.inputs.slice, &run.slicer_config),
            vertex_color_floor: map.inputs.slice.vertex_color_floor.unwrap_or(0.0),
            ..map_terrain::TerrainOptions::default()
        };
        let flat;
        let sampler = match run.terrain {
            Terrain::Blank(z) => {
                flat = GroundSampler::flat(z);
                &flat
            }
            Terrain::Land => &map.sampler,
        };
        let key = crate::cache::GroundKey {
            land: matches!(run.terrain, Terrain::Land)
                .then_some((manifest_digest.as_str(), map.worldspace.as_str())),
            window: &map.window,
            sampler,
            projection: &map.projection,
            calibration: &calibration_toml,
            palette: &palette_bytes,
            overlays: &overlays,
            options: &options,
        };
        let (ground, reused) = crate::cache::cached_ground(&store, &key, || match run.terrain {
            Terrain::Blank(_) => map_terrain::build_ground_from(
                &map_terrain::MemoryLand::default(),
                &map.window,
                sampler,
                &map.projection,
                &inputs.calibration,
                &palette_map,
                &overlays,
                &options,
            ),
            Terrain::Land => map_terrain::build_ground(
                &mir,
                &map.worldspace,
                &map.window,
                sampler,
                &map.projection,
                &inputs.calibration,
                &palette_map,
                &overlays,
                &options,
            ),
        })
        .map_err(|e| stage(format!("{}: {e}", run.label)))?;
        if reused {
            crate::timings::note("ground reused");
        }
        tile_carry = run.tile_cap.saturating_sub(ground.tiles.len());
        run.squares = ground
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
        run.ground = Some(ground);
    }

    // People: one set of sprite sets for the layer.
    let people_time = crate::timings::span("people and catalogue");
    let mut sets: BTreeMap<String, usize> = BTreeMap::new();
    let mut critter_art: Vec<map_builder::CritterArt> = Vec::new();
    for map in &mut plan.maps {
        let (mut critters, skipped) = map::critter_inputs_into(
            &map.inputs,
            &map.cells,
            &map.references,
            &mut sets,
            &mut critter_art,
        )?;
        if let Some([lo, hi]) = map.critter_band {
            let z_of: BTreeMap<String, f32> = map
                .references
                .iter()
                .map(|r| (r.form.to_string(), r.transform.position[2]))
                .collect();
            critters.retain(|c| z_of.get(&c.reference).is_some_and(|z| *z >= lo && *z <= hi));
        }
        map.critters = critters;
        map.people_skipped = skipped;
    }

    // Loot: one catalogue over every map's references and placements.
    let catalogue = if inputs.loot_on() {
        let rebalance = inputs.rebalance()?;
        let mut references: Vec<mir::Reference> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut art: Vec<map_slicer::ArtImage> = Vec::new();
        let mut placements: Vec<map_slicer::Placement> = Vec::new();
        let mut critters: Vec<map_builder::CritterPlacement> = Vec::new();
        for map in &plan.maps {
            for r in &map.references {
                if seen.insert(r.form.to_string()) {
                    references.push(r.clone());
                }
            }
            for run in &map.runs {
                let sliced = run.sliced.as_ref().expect("sliced above");
                let offset = art.len();
                art.extend(sliced.art.iter().cloned());
                placements.extend(sliced.placements.iter().map(|p| map_slicer::Placement {
                    art: p.art + offset,
                    ..p.clone()
                }));
            }
            critters.extend(map.critters.iter().cloned());
        }
        let built = crate::catalogue::build(&crate::catalogue::CatalogueInput {
            inputs,
            mir: &mir,
            rebalance: &rebalance,
            references: &references,
            art: &art,
            placements: &placements,
            critters: &critters,
            base: &base,
            palette_map: &palette_map,
            palette_bytes: &palette_bytes,
        })?;
        Some(built)
    } else {
        None
    };

    drop(people_time);
    // Protected hexes and footprints, per map over its elevations.
    for map in &mut plan.maps {
        let mut protected: BTreeSet<i32> = BTreeSet::new();
        let mut footprints = BTreeMap::new();
        for run in &map.runs {
            let sliced = run.sliced.as_ref().expect("sliced above");
            protected.extend(map::run_hexes(&sliced.placements));
            if map.footprints_on {
                footprints.extend(sliced.footprints.clone());
            }
        }
        map.build_options.protected_hexes = protected.into_iter().collect();
        if map.footprints_on {
            map.build_options.footprints = footprints;
        }
    }

    let layer_name = format!("Mojave Foundry: {}", layer.name);
    let layer_options = map_builder::LayerOptions {
        standalone: inputs.standalone,
        catalogue: catalogue
            .as_ref()
            .map(|c| std::sync::Arc::new(c.catalogue.clone())),
        close_unreached: layer.close_unreached,
        ..map_builder::LayerOptions::default()
    };
    let build_time = crate::timings::span("build the layer");
    let built = {
        let mut map_inputs = Vec::new();
        for map in &plan.maps {
            let elevations = map
                .runs
                .iter()
                .map(|run| map_builder::ElevationInput {
                    sampler: &map.sampler,
                    ground: map_builder::GroundTiles {
                        tiles: &run.ground.as_ref().expect("built above").tiles,
                        squares: &run.squares,
                    },
                    art: &run.art,
                    placements: &run.placements,
                    blocked_hexes: &run.sliced.as_ref().expect("sliced above").blocked_hexes,
                    interior: run.interior.clone(),
                    level: run.level.clone(),
                })
                .collect();
            let mut options = map.build_options.clone();
            options.standalone = inputs.standalone;
            map_inputs.push(map_builder::MapInput {
                map_name: &map.stem,
                map_number: None,
                window: map.window,
                projection: &map.projection,
                elevations,
                entrance_elevation: map.entrance_elevation,
                cells: &map.builder_cells,
                critters: &map.critters,
                options,
            });
        }
        map_builder::build_layer(&map_builder::LayerInput {
            layer_id: map::LAYER_ID,
            layer_name: &layer_name,
            base_files: &base,
            critter_art: &critter_art,
            maps: map_inputs,
            stairs: plan.stairs.clone(),
            options: layer_options,
        })
        .map_err(stage)?
    };
    drop(build_time);

    let write_time = crate::timings::span("write and verify the layer");
    if let Some(parent) = mod_dir.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    {
        let _t = crate::timings::span("write");
        built.layer.write(&mod_dir).map_err(stage)?;
    }
    let written = {
        // What is on disk is what a player loads: check that it is the
        // layer in memory, byte for byte, then verify that.
        let _t = crate::timings::span("check the written files");
        map::check_written(&mod_dir, &built.layer)?;
        &built.layer
    };
    let verified = {
        let _t = crate::timings::span("verify the layer");
        map_builder::verify_layer(written, &base).map_err(stage)?
    };
    let links = {
        let _t = crate::timings::span("verify the links");
        map_builder::verify_links(written, &base).map_err(stage)?
    };
    drop(write_time);
    let _report_time = crate::timings::span("previews, report, features");

    // Previews, features, the items page.
    let preview_time = crate::timings::span("previews");
    let gallery_dir = inputs.gallery_dir();
    std::fs::create_dir_all(&gallery_dir).map_err(io(&gallery_dir))?;
    for map in &plan.maps {
        for run in &map.runs {
            let (indexed, width, height) = map::compose_preview(
                run.ground.as_ref().expect("built above"),
                run.sliced.as_ref().expect("sliced above"),
            );
            let name = if run.elevation == 0 {
                format!("map-{}.png", map.stem)
            } else {
                format!("map-{}-e{}.png", map.stem, run.elevation)
            };
            let path = gallery_dir.join(name);
            let png =
                crate::cache::cached_png(&store, &indexed, width, height, &palette_bytes, || {
                    let rgba = quantize::to_rgba(&indexed, &palette);
                    foundry_gallery::encode_png(&rgba, width, height).map_err(stage)
                })?;
            std::fs::write(&path, png).map_err(io(&path))?;
        }
    }
    drop(preview_time);
    if let Some(catalogue) = &catalogue {
        let _t = crate::timings::span("items page");
        crate::gallery::items_page(inputs, &catalogue.report.items, &palette)?;
    }
    let features_dir = inputs.out.join(map::FEATURES_DIR);
    std::fs::create_dir_all(&features_dir).map_err(io(&features_dir))?;
    for (m, map) in plan.maps.iter().enumerate() {
        let built_map = &built.maps[m];
        let entrance = usize::from(built_map.entrance_elevation);
        let run = map
            .runs
            .iter()
            .find(|r| usize::from(r.elevation) == entrance)
            .unwrap_or(&map.runs[0]);
        let mut people: Vec<Person> = built_map
            .report
            .critters
            .iter()
            .zip(&built_map.critter_elevations)
            .filter(|(_, e)| usize::from(**e) == entrance)
            .map(|(c, _)| Person {
                name: map.critters[c.index].name.clone(),
                reference: c.reference.clone(),
                hex: c.hex,
            })
            .collect();
        map::rank_people(&mut people);
        let features = MapFeatures {
            map_file: built_map.file.clone(),
            entry_hex: built_map.report.entry_hex,
            features: map::features(run.sliced.as_ref().expect("sliced above")),
            interior: map.interior.as_ref().map(|s| s.cell.clone()),
            people: if map.interior.is_some() {
                people
            } else {
                Vec::new()
            },
            game: Some(inputs.game.id().to_owned()),
        };
        let json = serde_json::to_string_pretty(&features).map_err(stage)?;
        let stem = built_map
            .file
            .trim_start_matches("maps/")
            .trim_end_matches(".map");
        let path = features_dir.join(format!("{stem}.json"));
        std::fs::write(&path, json.clone() + "\n").map_err(io(&path))?;
        if m == 0 {
            std::fs::write(&features_path, json + "\n").map_err(io(&features_path))?;
        }
    }

    // The placement index beside the report, never inside the layer.
    {
        let _t = crate::timings::span("placement index");
        let (referenced, why) = crate::placements::referenced_of(&mir);
        if let Some(why) = why {
            crate::timings::note(format!("no referenced set: {why}"));
        }
        let stage_maps: Vec<crate::placements::StageMap<'_>> = plan
            .maps
            .iter()
            .zip(&built.maps)
            .map(|(map, built_map)| {
                crate::placements::StageMap::of_layer(
                    &map.stem,
                    &map.references,
                    map.runs
                        .iter()
                        .map(|run| (run.elevation, run.placements.as_slice()))
                        .collect(),
                    built_map,
                    catalogue.as_ref().map(|c| &c.catalogue),
                    map.people_skipped
                        .iter()
                        .map(|p| (p.reference.clone(), p.reason.clone()))
                        .chain(map.runs.iter().flat_map(|run| {
                            run.sliced.iter().flat_map(crate::placements::slicer_skips)
                        }))
                        .collect(),
                )
            })
            .collect();
        crate::placements::write_for_stage(
            &inputs.out,
            &mod_dir,
            inputs.game,
            &stage_maps,
            &referenced,
        )
        .map_err(stage)?;
    }

    let report = report_text(
        inputs,
        layer,
        &plan,
        &built,
        &budgets,
        &verified,
        &links,
        catalogue.as_ref().map(|c| &c.report),
        &critter_art,
        redirected,
        started.elapsed().as_secs_f64(),
    );
    let report_path = inputs.out.join("map-report.md");
    std::fs::write(&report_path, report).map_err(io(&report_path))?;

    let start = &built.maps[0];
    let summary = MapSummary {
        reused: false,
        map_file: start.file.clone(),
        map_number: start.number,
        references: plan.maps.iter().map(|m| m.references.len()).sum(),
        art_entries: plan
            .maps
            .iter()
            .flat_map(|m| &m.runs)
            .map(|r| r.art.len())
            .sum(),
        placements: plan
            .maps
            .iter()
            .flat_map(|m| &m.runs)
            .map(|r| r.placements.len())
            .sum(),
        floor_tiles: plan
            .maps
            .iter()
            .flat_map(|m| &m.runs)
            .map(|r| r.ground.as_ref().map_or(0, |g| g.tiles.len()))
            .sum(),
        walkable_hexes: built
            .maps
            .iter()
            .flat_map(|m| &m.elevations)
            .map(|e| e.walk.walkable)
            .sum(),
        blockers: built
            .maps
            .iter()
            .flat_map(|m| &m.elevations)
            .map(|e| e.blockers)
            .sum(),
        exits: built
            .maps
            .iter()
            .map(|m| m.report.exits.len() + m.links.len())
            .sum(),
        maps: start.report.maps.len(),
        layer_files: verified.frm
            + verified.art_lists
            + verified.prototype_lists
            + verified.prototypes
            + verified.messages
            + verified.maps.len(),
        critters: built.maps.iter().map(|m| m.report.critters.len()).sum(),
    };
    crate::timings::note(format!("cache store: {}", store.summary()));
    assets.note();
    let maps_dir = cache_dir.join("maps");
    std::fs::create_dir_all(&maps_dir).map_err(io(&maps_dir))?;
    for (stem, key) in &per_map_keys {
        let path = maps_dir.join(format!("{stem}.key"));
        std::fs::write(&path, key).map_err(io(&path))?;
    }
    std::fs::write(&summary_path, summary.to_cache()).map_err(io(&summary_path))?;
    std::fs::write(&key_path, key).map_err(io(&key_path))?;
    Ok(summary)
}

// --- The report -----------------------------------------------------------

fn elevation_reach(text: &mut String, e: &map_builder::ElevationReport) {
    let components = &e.components;
    let total: usize = components.sizes.iter().sum();
    let main = components
        .main
        .and_then(|m| components.sizes.get(m))
        .copied()
        .unwrap_or(0);
    let largest: Vec<String> = components
        .sizes
        .iter()
        .take(6)
        .map(ToString::to_string)
        .collect();
    let _ = writeln!(
        text,
        "- walkable {} hexes; {} components ({} hexes); the main one holds {} ({:.1} %); largest {}",
        e.walk.walkable,
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
        }
    );
    if let Some(entrance) = &e.entrance {
        let _ = writeln!(
            text,
            "- entrance hex {}: {:?}; in the largest component: {}",
            entrance.hex,
            entrance.sight,
            if entrance.in_main { "yes" } else { "no" }
        );
    }
    let _ = writeln!(
        text,
        "- passages {} ({} hexes opened); islands left {}; blockers {}; objects {}",
        e.passages.len(),
        e.opened_hexes,
        e.islands_left.len(),
        e.blockers,
        e.objects
    );
    for island in &e.islands_left {
        let _ = writeln!(
            text,
            "- island left: {} hexes at {} holding {}: {}",
            island.island_size,
            island.island_hex,
            island.holds.join(", "),
            island.reason
        );
    }
    if let Some((reached, planned)) = e.reached {
        let _ = writeln!(
            text,
            "- reached from the layer's start through links: {reached} of {planned} walkable hexes ({:.1} %)",
            if planned == 0 {
                0.0
            } else {
                100.0 * reached as f64 / planned as f64
            }
        );
    }
    for island in &e.islands {
        let way = if island.way.is_empty() {
            String::new()
        } else {
            let shown: Vec<String> = island
                .way
                .iter()
                .take(6)
                .map(|(hex, what)| format!("{hex} ({what})"))
                .collect();
            format!(
                "; in the way: {}{}",
                shown.join(", "),
                if island.way.len() > 6 { ", ..." } else { "" }
            )
        };
        let _ = writeln!(
            text,
            "- island {} hexes at {}: {}; {}{}{}",
            island.size,
            island.first_hex,
            if island.ground_joins {
                "the ground joins it to the main component"
            } else if island.joins_with_tolerance {
                "the ground keeps it apart under this coverage rule, the tolerance rule would join it"
            } else {
                "the ground keeps it apart"
            },
            match island.reached {
                Some(true) => "reached through links",
                Some(false) => "NOT reached through links",
                None => "reach unknown",
            },
            if island.holds.is_empty() {
                String::new()
            } else {
                format!("; holds {}", island.holds.join(", "))
            },
            way
        );
    }
    for (hex, size, holds) in &e.closed {
        let _ = writeln!(
            text,
            "- closed as unreached: {size} hexes at {hex}{}",
            if holds.is_empty() {
                String::new()
            } else {
                format!(", holding {}", holds.join(", "))
            }
        );
    }
    for perch in &e.perches {
        let _ = writeln!(
            text,
            "- occupied perch: {} hexes at hex {}, holds {} (not reached from the start)",
            perch.size,
            perch.hex,
            perch.holds.join(", ")
        );
    }
    match &e.lost_to_closing {
        Some(lost) if lost.is_empty() => {
            if !e.closed.is_empty() || !e.perches.is_empty() {
                let _ = writeln!(text, "- critters lost to closing: none");
            }
        }
        Some(lost) => {
            let _ = writeln!(text, "- critters LOST to closing: {}", lost.join(", "));
        }
        None => {}
    }
    for passage in &e.passages {
        let _ = writeln!(
            text,
            "- passage {} to {}: {} hexes, joins an island of {} holding {}",
            passage.from,
            passage.to,
            passage.length,
            passage.island_size,
            passage.holds.join(", ")
        );
    }
    for (what, hex, component) in &e.arrivals {
        let _ = writeln!(
            text,
            "- {what} on hex {hex}: {}",
            match component {
                Some(c) if Some(*c) == components.main => "in the main component".to_owned(),
                Some(c) => format!("component {c}, NOT the main one"),
                None => "on no walkable component".to_owned(),
            }
        );
    }
    let r = &e.readability;
    let _ = writeln!(
        text,
        "- readability: {} walkable hexes, clear {}, behind {}, covered {}; open floor {:.1} %",
        r.walkable_hexes,
        r.clear,
        r.behind,
        r.covered,
        100.0 * r.open_floor_ratio
    );
    let _ = writeln!(text, "- people here: {}", e.critters.len());
}

#[allow(clippy::too_many_arguments)]
fn report_text(
    inputs: &Inputs,
    layer: &LayerSpec,
    plan: &Plan,
    built: &map_builder::BuiltLayer,
    budgets: &Budgets,
    verified: &map_builder::VerifyReport,
    links: &map_builder::LinkCheck,
    catalogue: Option<&crate::catalogue::CatalogueReport>,
    critter_art: &[map_builder::CritterArt],
    redirected: usize,
    seconds: f64,
) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "# Map report: {} (layer)\n", layer.name);
    let _ = writeln!(
        text,
        "Game: {} (`{}`); slicer pattern profile `{}`.\n",
        inputs.game.display_name(),
        inputs.game.id(),
        crate::source::short_id(inputs.game)
    );
    let _ = writeln!(text, "## Layer\n");
    let _ = writeln!(
        text,
        "{} maps, {} stub maps, built in one layer ({}). Start map: `{}`.\n",
        built.maps.len(),
        built.stubs.len(),
        if inputs.standalone {
            "standalone: the walls and tiles lists replace Fallout 2's"
        } else {
            "extending Fallout 2's lists"
        },
        built.maps[0].file
    );
    let _ = writeln!(
        text,
        "| map | number | elevations | entrance | walkable per elevation | people per elevation | links | doors kept as stubs |\n|---|---:|---:|---|---|---|---:|---:|"
    );
    for map in &built.maps {
        let walkable: Vec<String> = map
            .elevations
            .iter()
            .map(|e| e.walk.walkable.to_string())
            .collect();
        let people: Vec<String> = map
            .elevations
            .iter()
            .map(|e| e.critters.len().to_string())
            .collect();
        let _ = writeln!(
            text,
            "| `{}` | {} | {} | hex {} on elevation {} | {} | {} | {} | {} |",
            map.file,
            map.number,
            map.elevations.len(),
            map.report.entry_hex,
            map.entrance_elevation,
            walkable.join(" / "),
            people.join(" / "),
            map.links.len(),
            map.report.exits.len()
        );
    }
    let _ = writeln!(text, "\n### Links\n");
    let mut both = 0;
    let mut one_way = 0;
    for map in &built.maps {
        for link in &map.links {
            if link.returns {
                both += 1;
            } else {
                one_way += 1;
            }
            let kind = match link.kind {
                map_builder::LinkKind::Door => "door",
                map_builder::LinkKind::Stairs => "stairs",
            };
            let _ = writeln!(
                text,
                "- {kind} {} on `{}` elevation {}{}: {} grids {} steps out -> `{}` (map {}) elevation {} hex {}; {}",
                link.from,
                map.file,
                link.elevation,
                link.door_hex.map_or(String::new(), |h| format!(" hex {h}")),
                link.grid_hexes.len(),
                link.grid_steps,
                link.to_file,
                link.to_number,
                link.to_elevation,
                link.to_hex,
                if link.returns { "both ways" } else { "ONE WAY" }
            );
        }
    }
    let _ = writeln!(
        text,
        "\nLink ends: {both} with a way back, {one_way} one way. `verify_links`: {} grids checked, {} links between map elevations, {} one way{}.\n",
        links.grids,
        links.links,
        links.one_way.len(),
        if links.one_way.is_empty() {
            String::new()
        } else {
            format!(": {}", links.one_way.join("; "))
        }
    );
    if redirected > 0 {
        let _ = writeln!(
            text,
            "{redirected} teleports named a cell no map is built from (a persistent cell) and were \
             pointed at the cell their partner door was extracted into.\n"
        );
    }
    let _ = writeln!(text, "Stubs (doors leading outside the build):\n");
    if built.stubs.is_empty() {
        let _ = writeln!(text, "- none");
    }
    for (file, number) in &built.stubs {
        let _ = writeln!(text, "- `{file}` (map {number})");
    }
    let _ = writeln!(text, "\n### Art budget, per map and elevation\n");
    let _ = writeln!(text, "{}", budget_text(plan, budgets));
    let _ = writeln!(
        text,
        "Shares are in proportion to the slicer's estimate (floor tiles: to each elevation's floor), \
         largest remainder; the budget column is the share plus what the runs sliced before left \
         unused (interiors first, then the levels above an exterior's ground, its ground level \
         last; tiles smallest first).\n"
    );
    if let Some(map) = built.maps.first() {
        let report = &map.report;
        map::write_list_section(&mut text, budgets, report, verified);
    }
    map::write_loot_section(&mut text, built.loot.as_ref(), catalogue);
    let _ = writeln!(text, "## Characters\n");
    if let Some(map) = built.maps.first() {
        map::write_critter_art(&mut text, &map.report, critter_art);
    }
    let _ = writeln!(text, "\nBuilt in {seconds:.0} s.\n");

    for (m, map) in plan.maps.iter().enumerate() {
        let b = &built.maps[m];
        let _ = writeln!(text, "# Map `{}` (map {})\n", b.file, b.number);
        let _ = writeln!(
            text,
            "Slice `{}`; window {} units square, north-west corner at ({}, {}).\n",
            map.inputs.slice.name, map.window.size, map.window.x_west, map.window.y_north
        );
        if !map.split_lines.is_empty() {
            let _ = writeln!(
                text,
                "## {}\n",
                if map.interior.is_some() {
                    "Storeys"
                } else {
                    "Levels"
                }
            );
            for line in &map.split_lines {
                let _ = writeln!(text, "- {line}");
            }
            let _ = writeln!(text);
        }
        for line in b.interior_summary() {
            let _ = writeln!(text, "- {line}");
        }
        if !b.report.interior_summary().is_empty() {
            let _ = writeln!(text);
        }
        for (e, run) in map.runs.iter().enumerate() {
            let sliced = run.sliced.as_ref().expect("sliced above");
            let _ = writeln!(text, "## Elevation {} ({})\n", run.elevation, run.what);
            if run.doors_demoted > 0 {
                let _ = writeln!(
                    text,
                    "- {} placements of doors of other elevations left out here",
                    run.doors_demoted
                );
            }
            if let Some(report) = b.elevations.get(e) {
                elevation_reach(&mut text, report);
            }
            let _ = writeln!(text);
            if let (Some(scene), Some(build)) = (&map.interior, &run.interior) {
                let _ = writeln!(
                    text,
                    "- floor z {:.1}, walk band {:.0} to {:.0}",
                    build.floor_z,
                    build.band()[0],
                    build.band()[1]
                );
                if let Some(i) = &sliced.report.interior {
                    let s = &i.settings;
                    let _ = writeln!(
                        text,
                        "- cut {} above the floor; floor baked: {} triangles, {} hexes; left out: {} above the cut, {} hanging, {} below the floor",
                        s.cut_height,
                        i.floor_triangles,
                        i.floor_hexes.len(),
                        i.above_cut,
                        i.hanging,
                        i.below_floor
                    );
                }
                let _ = scene;
            }
            let _ = writeln!(text, "### Art budget\n\n```\n{}\n```\n", sliced.report);
            map::write_slicer_sections(&mut text, sliced);
            if let Some(ground) = &run.ground {
                map::write_ground_section(&mut text, &ground.stats);
            }
            map::write_open_section(
                &mut text,
                &run.open,
                map.build_options.protected_hexes.len(),
            );
        }
        if m == 0 || map.declutter.profile != "off" {
            let first = map.runs[0].sliced.as_ref().expect("sliced above");
            map::write_declutter_section(&mut text, &map.declutter, first);
        }
        let _ = writeln!(text, "## Readability\n");
        for line in b.report.readability_summary() {
            let _ = writeln!(text, "- {line}");
        }
        let _ = writeln!(text, "\n## Furniture\n");
        if !map.footprints_on {
            let _ = writeln!(
                text,
                "- footprints off (slice key `footprints = true` turns them on)"
            );
        }
        for line in b.furniture.summary() {
            let _ = writeln!(text, "- {line}");
        }
        let _ = writeln!(text, "\n## Exits and links\n");
        for exit in &b.report.exits {
            let _ = writeln!(
                text,
                "- door {} at hex {} leads to the stub `{}` (map {}); grids {} steps out on {:?}; returns to hex {}",
                exit.door,
                exit.door_hex,
                exit.stub_file,
                exit.stub_number,
                exit.grid_steps,
                exit.grid_hexes,
                exit.return_hex
            );
        }
        for line in b.link_lines() {
            let _ = writeln!(text, "- {line}");
        }
        for skipped in &b.report.skipped_teleports {
            let _ = writeln!(
                text,
                "- door {} not converted: {}",
                skipped.door, skipped.reason
            );
        }
        let _ = writeln!(text, "\n## People\n");
        for (c, e) in b.report.critters.iter().zip(&b.critter_elevations) {
            let _ = writeln!(
                text,
                "- {} ({}) on elevation {e} hex {}, facing {}, moved {} steps",
                map.critters[c.index].name, c.reference, c.hex, c.orientation, c.steps
            );
        }
        for skipped in &b.report.skipped_critters {
            let _ = writeln!(
                text,
                "- {} ({}) not placed: {}",
                map.critters[skipped.index].name, skipped.reference, skipped.reason
            );
        }
        for person in &map.people_skipped {
            let _ = writeln!(
                text,
                "- {} ({}): {}",
                person.name, person.reference, person.reason
            );
        }
        for line in b.report.critters_summary() {
            let _ = writeln!(text, "- {line}");
        }
        let _ = writeln!(text, "\n## Notes\n");
        for note in &b.report.notes {
            let _ = writeln!(text, "- {note}");
        }
        let _ = writeln!(text);
    }
    let _ = writeln!(
        text,
        "# Parsed back through vault-formats\n\n```\n{verified:#?}\n```"
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_follow_the_demand_and_sum_to_the_total() {
        assert_eq!(shares(10, &[1, 1]), [5, 5]);
        assert_eq!(shares(10, &[3, 1]), [8, 2]);
        assert_eq!(shares(7, &[1, 1, 1]), [3, 2, 2]);
        assert_eq!(shares(4096, &[100]), [4096]);
        assert_eq!(shares(5, &[0, 0]), [3, 2]);
        let s = shares(4096, &[3861, 177, 900, 450]);
        assert_eq!(s.iter().sum::<usize>(), 4096);
        assert!(s[0] > s[2] && s[2] > s[3] && s[3] > s[1]);
    }

    #[test]
    fn automatic_stems_drop_the_worldspace_and_stay_distinct() {
        let mut taken = BTreeSet::new();
        let a = auto_stem("MegatonMoriartysSaloon", "MegatonWorld", &taken);
        assert_eq!(a, "moriarty");
        taken.insert(a);
        let b = auto_stem("MegatonClinic", "MegatonWorld", &taken);
        assert_eq!(b, "clinic");
        taken.insert(b);
        let c = auto_stem("MegatonClinic", "MegatonWorld", &taken);
        assert_eq!(c, "clinic02");
        assert_eq!(
            auto_stem("SuperDuperMart", "Wasteland", &BTreeSet::new()),
            "superdup"
        );
    }

    const EXTERIOR: &str = "name = \"town\"\nworldspace = \"TestWorld\"\nplugins = [\"Test.esm\"]\n\
                            x = [-1, 1]\ny = [-1, 1]\nmap_center = [0.0, 0.0]\nloot = true\n";

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        dir
    }

    #[test]
    fn a_layer_file_resolves_its_maps_with_overrides() {
        let dir = dir_with(&[("town.toml", EXTERIOR)]);
        let text = "name = \"town-layer\"\nstart = \"inn\"\n[all_maps]\nwire = true\n\
                    [[maps]]\nslice = \"town.toml\"\nmap = \"town\"\nlevels = \"auto\"\n\
                    [[maps]]\ninterior = \"TestInn\"\nmap = \"inn\"\ncut_height = 150.0\n";
        let layer = LayerSpec::from_toml(text, dir.path()).unwrap();
        assert_eq!(layer.maps.len(), 2);
        let town = &layer.maps[0].slice;
        assert_eq!(town.map_stem(), "town");
        assert_eq!(town.levels.as_deref(), Some("auto"));
        assert_eq!(town.wire, Some(true));
        let inn = &layer.maps[1].slice;
        assert_eq!(inn.interior.as_deref(), Some("TestInn"));
        assert_eq!(inn.cut_height, Some(150.0));
        assert_eq!(
            inn.storeys.as_deref(),
            Some("auto"),
            "an interior of a layer splits its storeys"
        );
        assert!(inn.place_disabled && inn.map_center.is_none() && inn.levels.is_none());
        assert_eq!(inn.loot, Some(true), "the exterior's keys");
        assert_eq!(inn.wire, Some(true), "and [all_maps]");
        let union = layer.union_slice();
        assert_eq!(union.name, "town-layer");
        assert_eq!(union.loot, Some(true));
        assert!(union.interior.is_none());
    }

    #[test]
    fn bad_layers_are_refused() {
        let dir = dir_with(&[("town.toml", EXTERIOR)]);
        for (text, expect) in [
            (
                "name = \"l\"\n[[maps]]\nslice = \"town.toml\"\n[[maps]]\nslice = \"town.toml\"\n",
                "share the stem",
            ),
            (
                "name = \"l\"\n[[maps]]\ninterior = \"Inn\"\n",
                "exterior map listed before",
            ),
            (
                "name = \"l\"\nstart = \"nowhere\"\n[[maps]]\nslice = \"town.toml\"\n",
                "names no map",
            ),
            (
                "name = \"l\"\n[[maps]]\nslice = \"town.toml\"\nstoreys = \"auto\"\n",
                "interior slice",
            ),
            (
                "name = \"l\"\n[[maps]]\nslice = \"missing.toml\"\n",
                "missing.toml",
            ),
            ("name = \"l\"\n", "at least one"),
        ] {
            let error = LayerSpec::from_toml(text, dir.path()).unwrap_err();
            assert!(error.contains(expect), "{text}: {error}");
        }
    }
}
