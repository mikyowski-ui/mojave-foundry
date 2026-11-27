//! Inputs every stage shares: the two installs, the output directory and
//! the slice being converted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vault_formats::{Dat2Archive, Palette, Vfs};

/// Everything that can stop a stage.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    /// The source install (`--game-dir`, `--fo3` or `--fnv`, whichever
    /// named it); `game` is the game it was taken for, when known.
    #[error("{flag} {path}: {reason}")]
    Source {
        flag: &'static str,
        game: Option<mir::GameVariant>,
        path: PathBuf,
        reason: String,
    },
    #[error("--f2 {path}: {reason}")]
    F2 { path: PathBuf, reason: String },
    #[error("--slice {name}: {reason}")]
    Slice { name: String, reason: String },
    #[error("{stage}: {reason}")]
    Stage { stage: &'static str, reason: String },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ConvertError {
    pub fn stage(stage: &'static str, reason: impl std::fmt::Display) -> Self {
        Self::Stage {
            stage,
            reason: reason.to_string(),
        }
    }
}

/// What to convert: a block of exterior cells of one worldspace, and the
/// window of it that becomes one Fallout 2 map, in either game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceSpec {
    pub name: String,
    /// The game the slice is for: `"fo3"` or `"fnv"` in a slice file.
    /// Unset, it is the game whose master `plugins` starts with (see
    /// [`SliceSpec::stated_game`]); a slice naming neither master leaves it
    /// to the command line and the install.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "game_key")]
    pub game: Option<mir::GameVariant>,
    /// Editor id of the worldspace.
    pub worldspace: String,
    /// Plugins in load order.
    pub plugins: Vec<String>,
    /// Inclusive exterior cell range, west to east.
    pub x: [i32; 2],
    /// Inclusive exterior cell range, south to north.
    pub y: [i32; 2],
    /// Teleport hops into interiors to extract.
    #[serde(default = "one")]
    pub interior_depth: u32,
    /// Centre of the map window in source units (`docs/contracts.md`
    /// section 4). Defaults to the centre of the cell range. On an interior
    /// slice it is in the room's own coordinates, and defaults to the
    /// centre of the chosen floor's navmesh.
    #[serde(default)]
    pub map_center: Option<[f32; 2]>,
    /// Build this interior cell (its editor id) as a standalone demo map
    /// instead of the exterior window. `x`, `y` and `worldspace` still say
    /// what to extract: the room is reached through a door of those cells
    /// (`interior_depth` at least 1). Needs `--standalone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interior: Option<String>,
    /// Interior only: the floor height (source units, the room's own
    /// coordinates). Default: the largest navmesh storey's floor, or the
    /// builder's dominant floor when the room has no storey.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor_z: Option<f32>,
    /// Interior only: place every person, including those the game data
    /// starts disabled (by their own flag or by an enable parent).
    #[serde(default, skip_serializing_if = "is_false")]
    pub place_disabled: bool,
    /// Interior only: people standing off the floor level are placed on
    /// the nearest walkable hex instead of skipped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clamp_levels: bool,
    /// Interior only: the slicer's ceiling cut above the floor (default
    /// 210).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_height: Option<f32>,
    /// Interior only: `[low, high]` above the floor in which up-facing
    /// geometry is baked into the floor (default `[-16, 32]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor_band: Option<[f32; 2]>,
    /// Interior only: a reference reaching above the cut whose lowest point
    /// is higher than this above the floor hangs from the ceiling and is
    /// left out (default 160).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hang_clearance: Option<f32>,
    /// Interior only: `[low, high]` around the floor (low at most 0, high
    /// at least 0) whose navmesh is walkable (default `[-40, 100]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk_band: Option<[f32; 2]>,
    /// Interior only: the source point `[x, y]` the player starts nearest.
    /// Default: the room's first teleport door (lowest form key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrance: Option<[f32; 2]>,
    /// `[declutter]`: the slicer's declutter and readability pass
    /// (`crate::declutter`). Absent, the game's default profile (off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declutter: Option<crate::declutter::DeclutterSpec>,
    /// `[tone]`: floor, props and figures tone curves over the profile's.
    /// Needs the declutter pass on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<crate::declutter::ToneSpec>,
    /// `loot = true`: items, filled containers, locked doors, armed and
    /// sided people (`crate::catalogue`). Absent or `false`: off, and the
    /// map is what it was before loot existed. `--loot` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loot: Option<bool>,
    /// The critter stage's animation level: `"minimal"` (the default: a
    /// stand, a walk and stills, byte for byte the first wave),
    /// `"combat"` or `"full"`. `--animations` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<String>,
    /// Falls and corpses: `"ragdoll"` (default) or `"game"`. Needs a level
    /// above minimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deaths: Option<String>,
    /// Walk travel against the grid: `"exact"` (default), `"matched"` or
    /// `"whole_hexes"`. Needs a level above minimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stride: Option<String>,
    /// How figures are drawn: `"default"` or `"readable"` (the figures
    /// tone and a dark rim). Needs a level above minimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub figure_look: Option<String>,
    /// `footprints = true`: block every hex under the placed bounds of
    /// furniture (single sprites, and small sliced references indoors).
    /// Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprints: Option<bool>,
    /// The map's file stem (1 to 8 of `[a-z0-9_-]`). Default: the slice's
    /// name, cut to eight characters. A layer needs distinct stems.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Interior only: `"one"` (the default outside a layer: one storey, as
    /// before) or `"auto"` (the default in a layer: every floor of the
    /// cell's navmesh its own elevation, `map_builder::storeys`, stairs as
    /// exit grids; more than three floors spill into further maps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storeys: Option<String>,
    /// Interior only, with `storeys = "auto"`: which floors (0 the lowest,
    /// as `map_builder::storeys::split_storeys` sorts them) become
    /// elevations 0, 1, 2, in this order. The others are left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storey_floors: Option<Vec<usize>>,
    /// Exterior only: `"one"` (the default: one level, as before) or
    /// `"auto"` (`map_builder::levels::split_levels`: surfaces over one
    /// another on different elevations, ramps as exit grids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<String>,
    /// `wire = true`: wire and lattice props (shopping carts) are drawn
    /// solid (`map_slicer::WireRule`); with it the Fallout 3 readable
    /// preset no longer prunes shopping carts as clutter. Default off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<bool>,
    /// Which hexes a navmesh triangle covers: `"tolerance"` (the default),
    /// `"centre"` or `"share:F"` (at least share `F` of the hex's pixels;
    /// `"share:0.5"` is recommended indoors with footprints).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navmesh_coverage: Option<String>,
    /// How the readability count judges a figure: `"any"` (the default,
    /// one pixel covers) or `"visible"` (more than a quarter of the box).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sight: Option<String>,
    /// How the entrance rule judges a figure: `"any"` or `"visible"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrance_sight: Option<String>,
    /// Exterior only: `navmesh_only = true` makes the hexes of cells
    /// without a navmesh not walkable when some cell of the window has one
    /// (`map_builder::BuildOptions::navmesh_only`): land the source never
    /// lets an actor walk, such as Megaton's ground outside its wall.
    /// Default off (the slope rule there, as before).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navmesh_only: Option<bool>,
    /// Exterior only: the darkest a land vertex colour may make the
    /// ground, 0 to 1 (`map_terrain::TerrainOptions::vertex_color_floor`;
    /// 0.4 on Megaton, whose crater's vertex shade quantizes to black).
    /// Default 0: the colours as stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertex_color_floor: Option<f32>,
    /// `coverage_bridges` (on by default): with a `navmesh_coverage` stricter than
    /// the tolerance rule, the islands it cuts out of one floor (a narrow
    /// flight of steps) are joined again through the hexes the tolerance
    /// rule covers (`map_builder::BuildOptions::coverage_bridges`).
    /// Default on; `false` leaves the islands as the strict rule cut them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_bridges: Option<bool>,
    /// Single-map builds: close the walkable components no walk from the
    /// entrance reaches, except those a critter stands on
    /// (`map_builder::BuildOptions::close_unreached`). Default on; `false`
    /// keeps them. A layer takes the layer file's key instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_unreached: Option<bool>,
    /// `enable_groups = true` (Pass 2, `docs/simulation-layer-spec.md` 8.3):
    /// a reference of an enable-parent group (it has an enable parent, or
    /// is one) that starts disabled is placed with the hidden flag instead
    /// of being left off, so a script can show it later. It blocks
    /// nothing while hidden and is not a placed person, item or
    /// container for the counts; the placement index lists it. Default
    /// off: such references are left out as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_groups: Option<bool>,
    /// `sim = true` (Pass 2, task T9): `all` runs the simulation stage
    /// (`crate::sim`) after the map stage, which needs nothing more than
    /// the run's output and the Fallout 2 install. Default off. A layer
    /// takes the layer file's top-level `sim` key as well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim: Option<bool>,
    /// `hostile_packet = <n>`: the AI packet number hostile people take
    /// when `loot` is on (`crate::catalogue`), instead of the rebalance
    /// table's (`convert.hostility.hostile_packet`, then
    /// `hostile_packet_number`). Must be a packet of the base `ai.txt`.
    /// Default: the table's, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostile_packet: Option<i32>,
}

/// `game = "fo3" | "fnv"` (the longer ids `fallout3` and `falloutnv` are
/// read too); written as the short form.
mod game_key {
    use mir::GameVariant;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(game: &Option<GameVariant>, s: S) -> Result<S::Ok, S::Error> {
        match game {
            Some(game) => s.serialize_str(crate::source::short_id(*game)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<GameVariant>, D::Error> {
        let text = String::deserialize(d)?;
        text.parse::<GameVariant>().map(Some).map_err(|_| {
            serde::de::Error::custom(format!("game {text:?}: expected \"fo3\" or \"fnv\""))
        })
    }
}

fn one() -> u32 {
    1
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Default for SliceSpec {
    /// An empty exterior slice: no name, no worldspace, no plugins (so not
    /// valid as it stands), cell (0, 0), interior depth 1, every interior
    /// setting off.
    fn default() -> Self {
        Self {
            name: String::new(),
            game: None,
            worldspace: String::new(),
            plugins: Vec::new(),
            x: [0, 0],
            y: [0, 0],
            interior_depth: one(),
            map_center: None,
            interior: None,
            floor_z: None,
            place_disabled: false,
            clamp_levels: false,
            cut_height: None,
            floor_band: None,
            hang_clearance: None,
            walk_band: None,
            entrance: None,
            declutter: None,
            tone: None,
            loot: None,
            animations: None,
            deaths: None,
            stride: None,
            figure_look: None,
            footprints: None,
            map: None,
            storeys: None,
            storey_floors: None,
            levels: None,
            wire: None,
            navmesh_coverage: None,
            sight: None,
            entrance_sight: None,
            navmesh_only: None,
            vertex_color_floor: None,
            coverage_bridges: None,
            close_unreached: None,
            enable_groups: None,
            sim: None,
            hostile_packet: None,
        }
    }
}

/// The built-in slices: name and game. The first of each game is its
/// default.
pub const PRESETS: &[(&str, mir::GameVariant)] = &[
    ("goodsprings", mir::GameVariant::FalloutNewVegas),
    ("springvale", mir::GameVariant::Fallout3),
];

/// Springvale (Fallout 3): the town's cells and its window centre, the
/// same as `slices/fo3/springvale.toml`.
const SPRINGVALE_X: [i32; 2] = [-5, 0];
const SPRINGVALE_Y: [i32; 2] = [-6, -1];
const SPRINGVALE_CENTER: [f32; 2] = [-7_100.0, -11_200.0];

impl SliceSpec {
    /// The built-in slice a run of `game` converts when `--slice` is not
    /// given: `goodsprings` for New Vegas, `springvale` for Fallout 3.
    pub fn default_name(game: mir::GameVariant) -> &'static str {
        PRESETS
            .iter()
            .find(|(_, g)| *g == game)
            .map_or("goodsprings", |(name, _)| name)
    }

    /// The built-in slices, by name. `goodsprings` is New Vegas's,
    /// `springvale` Fallout 3's (also `slices/fo3/springvale.toml`).
    pub fn preset(name: &str) -> Option<Self> {
        match name {
            "goodsprings" => Some(Self {
                name: "goodsprings".to_owned(),
                worldspace: "WastelandNV".to_owned(),
                plugins: vec!["FalloutNV.esm".to_owned()],
                x: [-21, -14],
                y: [-4, 3],
                interior_depth: 1,
                // The centre of exterior cell (-18, 0).
                map_center: Some([-71_680.0, 2_048.0]),
                ..Self::default()
            }),
            "springvale" => Some(Self {
                name: "springvale".to_owned(),
                game: Some(mir::GameVariant::Fallout3),
                worldspace: "Wasteland".to_owned(),
                plugins: vec!["Fallout3.esm".to_owned()],
                x: SPRINGVALE_X,
                y: SPRINGVALE_Y,
                interior_depth: 1,
                map_center: Some(SPRINGVALE_CENTER),
                loot: Some(true),
                animations: Some("combat".to_owned()),
                ..Self::default()
            }),
            _ => None,
        }
    }

    /// A preset name, or the path of a TOML file with the same fields.
    pub fn resolve(name: &str) -> Result<Self, ConvertError> {
        if let Some(preset) = Self::preset(name) {
            return Ok(preset);
        }
        let path = Path::new(name);
        if !path.is_file() {
            return Err(ConvertError::Slice {
                name: name.to_owned(),
                reason: format!(
                    "not a built-in slice (known: {}) and not a file",
                    PRESETS
                        .iter()
                        .map(|(n, g)| format!("{n} for {}", g.display_name()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        let text = std::fs::read_to_string(path).map_err(|source| ConvertError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml(&text).map_err(|reason| ConvertError::Slice {
            name: name.to_owned(),
            reason,
        })
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let spec: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a slice spec is plain data")
    }

    fn validate(&self) -> Result<(), String> {
        let safe = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        if !safe(&self.name) {
            return Err("name must be letters, digits, '-' or '_'".to_owned());
        }
        if self.worldspace.is_empty() {
            return Err("worldspace is empty".to_owned());
        }
        if self.hostile_packet.is_some_and(|n| n < 0) {
            return Err("hostile_packet must be an AI packet number, 0 or more".to_owned());
        }
        if self.plugins.is_empty() {
            return Err("plugins is empty".to_owned());
        }
        if let (Some(game), Some(master)) = (self.game, self.plugins_game())
            && game != master
        {
            return Err(format!(
                "game = \"{}\" ({}), but plugins start with {}, the master of {}",
                crate::source::short_id(game),
                game.display_name(),
                self.plugins[0],
                master.display_name()
            ));
        }
        for (axis, range) in [("x", self.x), ("y", self.y)] {
            if range[0] > range[1] {
                return Err(format!("{axis} range is reversed"));
            }
            if i64::from(range[1]) - i64::from(range[0]) >= 64 {
                return Err(format!("{axis} range spans more than 64 cells"));
            }
        }
        if let Some(center) = self.map_center
            && !center.iter().all(|v| v.is_finite())
        {
            return Err("map_center is not finite".to_owned());
        }
        if let Some(declutter) = &self.declutter {
            declutter.validate(self.tone.as_ref())?;
        }
        self.critter_settings()?;
        self.validate_interior()
    }

    /// The keys added with layers (`map`, `storeys`, `storey_floors`,
    /// `levels`, `navmesh_coverage`, `sight`, `entrance_sight`).
    fn validate_layer_keys(&self) -> Result<(), String> {
        if let Some(map) = &self.map
            && !((1..=8).contains(&map.len())
                && map.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                }))
        {
            return Err(format!(
                "map {map:?}: 1 to 8 lowercase letters, digits, '_' or '-'"
            ));
        }
        for (key, value) in [("storeys", &self.storeys), ("levels", &self.levels)] {
            if let Some(v) = value
                && v != "one"
                && v != "auto"
            {
                return Err(format!("{key} = {v:?}: expected \"one\" or \"auto\""));
            }
        }
        if self.storey_floors.is_some() && self.storeys.as_deref() == Some("one") {
            return Err("storey_floors needs storeys = \"auto\"".to_owned());
        }
        if let Some(floors) = &self.storey_floors {
            let unique: std::collections::BTreeSet<&usize> = floors.iter().collect();
            if floors.is_empty() || floors.len() > 3 || unique.len() != floors.len() {
                return Err("storey_floors: one to three distinct floor indices".to_owned());
            }
        }
        if let Some(text) = &self.navmesh_coverage {
            parse_coverage(text)?;
        }
        for text in [&self.sight, &self.entrance_sight].into_iter().flatten() {
            parse_sight(text)?;
        }
        if self.interior.is_some()
            && (self.navmesh_only.is_some() || self.vertex_color_floor.is_some())
        {
            return Err(
                "navmesh_only and vertex_color_floor apply to an exterior slice".to_owned(),
            );
        }
        if let Some(v) = self.vertex_color_floor
            && !(v.is_finite() && (0.0..=1.0).contains(&v))
        {
            return Err(format!("vertex_color_floor = {v}: expected 0 to 1"));
        }
        if self.interior.is_some() && self.levels.is_some() {
            return Err("levels applies to an exterior slice (storeys to an interior)".to_owned());
        }
        if self.interior.is_none() && (self.storeys.is_some() || self.storey_floors.is_some()) {
            return Err(
                "storeys and storey_floors apply to an interior slice (levels to an exterior)"
                    .to_owned(),
            );
        }
        Ok(())
    }

    /// The map file stem: `map`, else the name's first eight characters.
    pub fn map_stem(&self) -> String {
        self.map
            .clone()
            .unwrap_or_else(|| crate::map::map_stem(&self.name))
    }

    /// The builder's coverage rule (`navmesh_coverage`), default
    /// tolerance.
    pub fn coverage(&self) -> map_builder::walk::NavmeshCoverage {
        self.navmesh_coverage
            .as_deref()
            .and_then(|t| parse_coverage(t).ok())
            .unwrap_or(map_builder::walk::NavmeshCoverage::Tolerance)
    }

    /// The builder's readability and entrance sight rules.
    pub fn sight_rules(&self) -> (map_builder::cover::SightRule, map_builder::cover::SightRule) {
        let rule = |t: &Option<String>| {
            t.as_deref()
                .and_then(|t| parse_sight(t).ok())
                .unwrap_or(map_builder::cover::SightRule::AnyPixel)
        };
        (rule(&self.sight), rule(&self.entrance_sight))
    }

    /// Whether this slice is built through the layer path on its own: its
    /// storeys or levels split (a one-map layer).
    pub fn wants_layer(&self) -> bool {
        self.storeys.as_deref() == Some("auto") || self.levels.as_deref() == Some("auto")
    }

    fn validate_interior(&self) -> Result<(), String> {
        self.validate_layer_keys()?;
        let Some(id) = &self.interior else {
            // `entrance` also names an exterior map's entrance target.
            let set: Vec<&str> = [
                ("floor_z", self.floor_z.is_some()),
                ("place_disabled", self.place_disabled),
                ("clamp_levels", self.clamp_levels),
                ("cut_height", self.cut_height.is_some()),
                ("floor_band", self.floor_band.is_some()),
                ("hang_clearance", self.hang_clearance.is_some()),
                ("walk_band", self.walk_band.is_some()),
            ]
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(key, _)| key)
            .collect();
            if self
                .entrance
                .is_some_and(|p| !p.iter().all(|v| v.is_finite()))
            {
                return Err("entrance is not finite".to_owned());
            }
            if set.is_empty() {
                return Ok(());
            }
            return Err(format!(
                "{} only apply to an interior slice (set interior = \"<editor id>\")",
                set.join(", ")
            ));
        };
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(
                "interior must be a cell editor id: letters, digits, '-' or '_'".to_owned(),
            );
        }
        if self.interior_depth < 1 {
            return Err(
                "an interior slice needs interior_depth of at least 1: extract reaches the room \
                 through a door of the cells"
                    .to_owned(),
            );
        }
        if self.floor_z.is_some_and(|z| !z.is_finite()) {
            return Err("floor_z is not finite".to_owned());
        }
        if self
            .entrance
            .is_some_and(|p| !p.iter().all(|v| v.is_finite()))
        {
            return Err("entrance is not finite".to_owned());
        }
        let settings = self.interior_slice(0.0);
        let cut = settings.cut_height;
        if !(cut.is_finite() && cut > 0.0) {
            return Err("cut_height must be a positive number".to_owned());
        }
        let [low, high] = settings.floor_band;
        if !(low.is_finite() && high.is_finite() && low <= high && high < cut) {
            return Err(
                "floor_band must be [low, high] rising, with high under cut_height".to_owned(),
            );
        }
        if self
            .hang_clearance
            .is_some_and(|h| !(h.is_finite() && h >= 0.0))
        {
            return Err("hang_clearance must be a number of at least 0".to_owned());
        }
        if let Some([low, high]) = self.walk_band
            && !(low.is_finite() && high.is_finite() && low <= 0.0 && high >= 0.0)
        {
            return Err(
                "walk_band must be [low, high] with low at most 0 and high at least 0".to_owned(),
            );
        }
        Ok(())
    }

    /// The slice's critter stage keys, parsed (each `None` when absent).
    pub fn critter_settings(&self) -> Result<CritterSettings, String> {
        let level = self
            .animations
            .as_deref()
            .map(|t| t.parse::<crate::critters::AnimationLevel>())
            .transpose()
            .map_err(|e| format!("animations = {e}"))?;
        let deaths = self.deaths.as_deref().map(parse_deaths).transpose()?;
        let stride = self.stride.as_deref().map(parse_stride).transpose()?;
        let look = self.figure_look.as_deref().map(parse_look).transpose()?;
        Ok(CritterSettings {
            level,
            deaths,
            stride,
            look,
            only: Vec::new(),
        })
    }

    /// The game whose master the plugin list starts with, if it does.
    pub fn plugins_game(&self) -> Option<mir::GameVariant> {
        self.plugins
            .first()
            .and_then(|p| mir::GameVariant::from_master_name(p))
    }

    /// The game this slice is for: its `game` key, else the game whose
    /// master its plugins start with. `None` for a slice that says nothing
    /// (a synthetic plugin list).
    pub fn stated_game(&self) -> Option<mir::GameVariant> {
        self.game.or_else(|| self.plugins_game())
    }

    /// Whether this slice builds an interior room.
    pub fn is_interior(&self) -> bool {
        self.interior.is_some()
    }

    /// The slicer's interior settings for a floor at `floor_z`: the
    /// defaults of `map_slicer::InteriorSlice::at`, overridden by the
    /// slice's own.
    pub fn interior_slice(&self, floor_z: f32) -> map_slicer::InteriorSlice {
        let mut slice = map_slicer::InteriorSlice::at(floor_z);
        if let Some(cut) = self.cut_height {
            slice.cut_height = cut;
        }
        if let Some(band) = self.floor_band {
            slice.floor_band = band;
        }
        if let Some(hang) = self.hang_clearance {
            slice.hang_clearance = hang;
        }
        slice
    }

    /// The builder's interior settings for a floor at `floor_z` (the
    /// entrance target is left to the caller).
    pub fn interior_build(&self, floor_z: f32) -> map_builder::InteriorBuild {
        let mut build = map_builder::InteriorBuild::new(floor_z);
        if let Some([low, high]) = self.walk_band {
            build.below = -low;
            build.above = high;
        }
        build.clamp_levels = self.clamp_levels;
        build
    }

    /// The map window's centre in source units. On an interior slice
    /// without `map_center` this is the cell range's centre, which says
    /// nothing about the room: the map stage takes the floor's navmesh
    /// centre instead.
    pub fn center(&self) -> [f32; 2] {
        self.map_center.unwrap_or_else(|| {
            let mid = |r: [i32; 2]| (r[0] + r[1] + 1) as f32 * 0.5 * 4096.0;
            [mid(self.x), mid(self.y)]
        })
    }

    pub fn mir_slice(&self) -> mir::Slice {
        mir::Slice {
            name: self.name.clone(),
            worldspace: self.worldspace.clone(),
            x: self.x[0]..=self.x[1],
            y: self.y[0]..=self.y[1],
            interior_depth: self.interior_depth,
        }
    }
}

/// The critter stage's settings from the command line or the slice
/// (`None`: not given there).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CritterSettings {
    pub level: Option<crate::critters::AnimationLevel>,
    pub deaths: Option<crate::critters::DeathSource>,
    pub stride: Option<crate::critters::Stride>,
    /// `true` for the readable look.
    pub look: Option<bool>,
    /// `--only`: editor ids to render.
    pub only: Vec<String>,
}

impl CritterSettings {
    /// `self` where given, else `other`.
    pub fn or(&self, other: &CritterSettings) -> CritterSettings {
        CritterSettings {
            level: self.level.or(other.level),
            deaths: self.deaths.or(other.deaths),
            stride: self.stride.or(other.stride),
            look: self.look.or(other.look),
            only: if self.only.is_empty() {
                other.only.clone()
            } else {
                self.only.clone()
            },
        }
    }
}

/// `ragdoll` or `game`.
pub fn parse_deaths(text: &str) -> Result<crate::critters::DeathSource, String> {
    match text {
        "ragdoll" => Ok(crate::critters::DeathSource::Ragdoll),
        "game" => Ok(crate::critters::DeathSource::Game),
        other => Err(format!("deaths {other:?}: expected ragdoll or game")),
    }
}

/// `exact`, `matched` or `whole_hexes` (`whole-hexes` too).
pub fn parse_stride(text: &str) -> Result<crate::critters::Stride, String> {
    match text {
        "exact" => Ok(crate::critters::Stride::Exact),
        "matched" => Ok(crate::critters::Stride::Matched),
        "whole_hexes" | "whole-hexes" => Ok(crate::critters::Stride::WholeHexes),
        other => Err(format!(
            "stride {other:?}: expected exact, matched or whole_hexes"
        )),
    }
}

/// `tolerance`, `centre` (`center`) or `share:F` with `F` in (0, 1].
pub fn parse_coverage(text: &str) -> Result<map_builder::walk::NavmeshCoverage, String> {
    use map_builder::walk::NavmeshCoverage;
    match text {
        "tolerance" => Ok(NavmeshCoverage::Tolerance),
        "centre" | "center" => Ok(NavmeshCoverage::Centre),
        other => {
            let share = other
                .strip_prefix("share:")
                .and_then(|s| s.trim().parse::<f32>().ok())
                .filter(|s| s.is_finite() && *s > 0.0 && *s <= 1.0)
                .ok_or_else(|| {
                    format!(
                        "navmesh_coverage {other:?}: expected \"tolerance\", \"centre\" or \
                         \"share:F\" with F in (0, 1]"
                    )
                })?;
            Ok(NavmeshCoverage::Share(share))
        }
    }
}

/// `any` (one pixel covers) or `visible` (`SightRule::VISIBLE`).
pub fn parse_sight(text: &str) -> Result<map_builder::cover::SightRule, String> {
    match text {
        "any" => Ok(map_builder::cover::SightRule::AnyPixel),
        "visible" => Ok(map_builder::cover::SightRule::VISIBLE),
        other => Err(format!("sight {other:?}: expected \"any\" or \"visible\"")),
    }
}

/// `default` (false) or `readable` (true).
pub fn parse_look(text: &str) -> Result<bool, String> {
    match text {
        "default" => Ok(false),
        "readable" => Ok(true),
        other => Err(format!(
            "figure look {other:?}: expected default or readable"
        )),
    }
}

/// The validated inputs of a run. Both installs are required for every
/// stage: the source game for content, Fallout 2 for the palette, fonts,
/// interface art and sound effects the output relies on.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// The source install's `Data` directory.
    pub data_dir: PathBuf,
    /// The source game (`crate::source::resolve`).
    pub game: mir::GameVariant,
    /// The Fallout 2 install directory.
    pub f2: PathBuf,
    /// Its main archive. `None` for an install that keeps everything as
    /// loose files under `data/` (the synthetic test install does).
    pub f2_master: Option<PathBuf>,
    pub out: PathBuf,
    pub slice: SliceSpec,
    pub jobs: usize,
    /// Scale, supersampling and lighting (`docs/contracts.md` section 4).
    pub calibration: foundry_render::Calibration,
    /// Which render path to use. The CPU path is the default because only
    /// it gives the same bytes on every machine.
    pub backend: foundry_render::Backend,
    /// Draw a one-pixel dark inner edge on sprites.
    pub outline: bool,
    /// Standalone mode (owner decision, 2026-11-22, for demonstration
    /// layers): the layer's wall and floor-tile lists replace Fallout 2's
    /// instead of extending them, so nearly all 4,096 entries of each are
    /// the map's. Stock maps draw the wrong walls and floors while such a
    /// layer is mounted. Default `false`; only the map stage reads it.
    pub standalone: bool,
    /// `--declutter off|readable`: overrides the slice's declutter profile.
    /// `None` leaves it to the slice and the game's default.
    pub declutter: Option<String>,
    /// `--loot on|off`: overrides the slice's `loot`. `None` leaves it to
    /// the slice (default off).
    pub loot: Option<bool>,
    /// `--rebalance <file>`: the rebalance tables (`crate::rebalance`).
    /// `None` looks for `rebalance/<game>.toml` beside the slice.
    pub rebalance: Option<PathBuf>,
    /// The slice file, when `--slice` named one: the default rebalance
    /// file is looked for beside it.
    pub slice_file: Option<PathBuf>,
    /// `--animations`, `--deaths`, `--stride`, `--figure-look`, `--only`:
    /// over the slice's keys.
    pub critter: CritterSettings,
    /// A layer of several maps (`--slice <file>.layer.toml`,
    /// `crate::layer`); `slice` is then the union the extract, sprite and
    /// critter stages run over. `None`: one slice, one map, as before.
    pub layer: Option<crate::layer::LayerSpec>,
    /// `--with-interiors`: build the exterior slice with every interior its
    /// doors lead to (depth 1) into one layer.
    pub with_interiors: bool,
    /// `--cache-dir`: where the map stage keeps its renders and grounds
    /// (`crate::cache`). `None`: `<out>/cache`.
    pub cache_dir: Option<PathBuf>,
    /// The `[sim]` table of the `--config` file (`foundry.toml`): the
    /// simulation stage's build options. Defaults: the owner decisions.
    pub sim: transpile::ir::SimConfig,
}

impl Inputs {
    /// The inputs for a source install whose game is decided from the
    /// slice and the install (`--game-dir` without `--game`).
    pub fn new(
        source: &Path,
        f2: &Path,
        out: &Path,
        slice: SliceSpec,
        jobs: usize,
    ) -> Result<Self, ConvertError> {
        Self::for_install(
            &crate::source::Install::any(source, None),
            f2,
            out,
            slice,
            jobs,
        )
    }

    /// The inputs for `install`; the game is decided by
    /// [`crate::source::resolve`].
    pub fn for_install(
        install: &crate::source::Install,
        f2: &Path,
        out: &Path,
        slice: SliceSpec,
        jobs: usize,
    ) -> Result<Self, ConvertError> {
        let resolved = crate::source::resolve(install, Some(&slice))?;
        let f2_master = find_master(f2)?;
        Ok(Self {
            data_dir: resolved.data_dir,
            game: resolved.game,
            f2: f2.to_owned(),
            f2_master,
            out: out.to_owned(),
            slice,
            jobs: jobs.max(1),
            calibration: foundry_render::Calibration::default(),
            backend: foundry_render::Backend::Cpu,
            outline: false,
            standalone: false,
            declutter: None,
            loot: None,
            rebalance: None,
            slice_file: None,
            critter: CritterSettings::default(),
            layer: None,
            with_interiors: false,
            cache_dir: None,
            sim: transpile::ir::SimConfig::default(),
        })
    }

    /// The layer this run builds, if any: the layer file's, or the
    /// exterior slice with `--with-interiors`, or a slice that splits its
    /// storeys or levels (a one-map layer).
    pub fn layer_spec(&self) -> Option<crate::layer::LayerSpec> {
        if let Some(layer) = &self.layer {
            return Some(layer.clone());
        }
        if self.with_interiors && !self.slice.is_interior() {
            return Some(crate::layer::LayerSpec::with_interiors_of(&self.slice));
        }
        self.slice
            .wants_layer()
            .then(|| crate::layer::LayerSpec::single(&self.slice))
    }

    /// These inputs for one map of a layer: its slice, everything else the
    /// run's.
    pub fn for_map(&self, slice: &SliceSpec) -> Inputs {
        Inputs {
            slice: slice.clone(),
            layer: None,
            with_interiors: false,
            ..self.clone()
        }
    }

    /// Whether enable-group children that start disabled are placed hidden
    /// (the slice's `enable_groups`, default off).
    pub fn enable_groups_on(&self) -> bool {
        self.slice.enable_groups.unwrap_or(false)
    }

    /// Whether this run builds items and loot: `--loot`, else the slice's
    /// `loot`, else off.
    pub fn loot_on(&self) -> bool {
        self.loot.or(self.slice.loot).unwrap_or(false)
    }

    /// The critter settings of this run: the command line's, else the
    /// slice's.
    pub fn critter_settings(&self) -> Result<CritterSettings, ConvertError> {
        let slice = self
            .slice
            .critter_settings()
            .map_err(|reason| ConvertError::Slice {
                name: self.slice.name.clone(),
                reason,
            })?;
        let mut settings = self.critter.or(&slice);
        // Owner decision (2026-11-23): the readable profile draws figures
        // with the readable look (the figures tone and a dark rim) unless
        // the flag or the slice names a look (`figure_look = "default"`
        // turns it off). Only above the minimal level, which draws figures
        // as the first wave did; with the pass off nothing changes.
        let above_minimal = settings
            .level
            .is_some_and(|l| l != crate::critters::AnimationLevel::Minimal);
        if settings.look.is_none() && above_minimal && self.readable_anywhere() {
            settings.look = Some(true);
        }
        Ok(settings)
    }

    /// Whether the declutter pass is `readable` for this run: the command
    /// line's profile, else on a layer any map's slice, else the slice's.
    pub fn readable_anywhere(&self) -> bool {
        if let Some(profile) = &self.declutter {
            return profile == "readable";
        }
        match &self.layer {
            Some(layer) => layer.maps.iter().any(|m| {
                m.slice
                    .declutter
                    .as_ref()
                    .and_then(|d| d.profile_name())
                    .unwrap_or_else(|| crate::declutter::default_profile(self.game))
                    == "readable"
            }),
            None => self.declutter_profile() == "readable",
        }
    }

    /// The rebalance tables of this run (`crate::rebalance`): `--rebalance`,
    /// else `rebalance/<game>.toml` beside the slice file (or the working
    /// directory) or above, else the built-in defaults.
    pub fn rebalance(&self) -> Result<crate::rebalance::LoadedRebalance, ConvertError> {
        let error = |reason: String| ConvertError::stage("rebalance", reason);
        if let Some(path) = &self.rebalance {
            return crate::rebalance::LoadedRebalance::from_file(self.game, path).map_err(error);
        }
        let start = self
            .slice_file
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        match crate::rebalance::find_default(self.game, &start) {
            Some(path) => {
                crate::rebalance::LoadedRebalance::from_file(self.game, &path).map_err(error)
            }
            None => Ok(crate::rebalance::LoadedRebalance::built_in(self.game)),
        }
    }

    /// The declutter profile name this run uses: `--declutter`, else the
    /// slice's `[declutter] profile`, else the game's default.
    pub fn declutter_profile(&self) -> String {
        self.declutter
            .clone()
            .or_else(|| {
                self.slice
                    .declutter
                    .as_ref()
                    .and_then(|d| d.profile_name().map(str::to_owned))
            })
            .unwrap_or_else(|| crate::declutter::default_profile(self.game).to_owned())
    }

    /// The inputs for `install` and a `--slice` argument (a built-in name
    /// or a TOML file), or the game's default slice when `slice` is `None`.
    pub fn from_args(
        install: &crate::source::Install,
        slice: Option<&str>,
        f2: &Path,
        out: &Path,
        jobs: usize,
    ) -> Result<Self, ConvertError> {
        // A layer file (`*.layer.toml`): its maps' union is the run's slice.
        let layer = match slice {
            Some(name) if crate::layer::is_layer_file(name) => {
                Some(crate::layer::LayerSpec::load(Path::new(name))?)
            }
            _ => None,
        };
        let spec = match (&layer, slice) {
            (Some(layer), _) => layer.union_slice(),
            (None, Some(name)) => SliceSpec::resolve(name)?,
            (None, None) => {
                let game = crate::source::resolve(install, None)?.game;
                SliceSpec::resolve(SliceSpec::default_name(game))?
            }
        };
        let mut inputs = Self::for_install(install, f2, out, spec, jobs)?;
        inputs.layer = layer;
        inputs.slice_file = slice
            .filter(|name| SliceSpec::preset(name).is_none())
            .map(PathBuf::from);
        Ok(inputs)
    }

    /// Replaces the default calibration with the one in a TOML file.
    pub fn with_calibration_file(mut self, path: &Path) -> Result<Self, ConvertError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConvertError::Io {
            path: path.to_owned(),
            source,
        })?;
        let (calibration, sim) = split_config(&text)
            .map_err(|e| ConvertError::stage("config", format!("{}: {e}", path.display())))?;
        self.calibration = calibration;
        self.sim = sim;
        Ok(self)
    }

    /// Whether `all` runs the simulation stage: the layer file's `sim`, or
    /// the slice's (default off).
    pub fn sim_on(&self) -> bool {
        self.layer.as_ref().is_some_and(|l| l.sim) || self.slice.sim.unwrap_or(false)
    }

    /// An interior slice builds a standalone demo map only: without
    /// `--standalone` it is refused, before any stage runs.
    pub fn check_mode(&self) -> Result<(), ConvertError> {
        // Declutter keys with the pass off are refused here too, before
        // extract, not only by the map stage.
        if self.declutter_profile() == "off" {
            crate::declutter::profile(
                self.game,
                "off",
                self.slice.declutter.as_ref(),
                self.slice.tone.as_ref(),
                &std::collections::BTreeSet::new(),
            )
            .map_err(|reason| ConvertError::Slice {
                name: self.slice.name.clone(),
                reason,
            })?;
        }
        // A figure look needs a level above minimal; refused here, before
        // extract, as the critter stage would refuse it.
        let settings = self.critter_settings()?;
        let minimal = settings
            .level
            .is_none_or(|l| l == crate::critters::AnimationLevel::Minimal);
        let toned = self
            .slice
            .tone
            .as_ref()
            .is_some_and(|t| t.figures.is_some());
        if minimal && (settings.look == Some(true) || toned) {
            return Err(ConvertError::Slice {
                name: self.slice.name.clone(),
                reason: "a figure look (figure_look = \"readable\", [tone.figures]) needs \
                         animations = \"combat\" or \"full\""
                    .to_owned(),
            });
        }
        // Every map of a layer is checked as its own slice would be; an
        // interior of a layer shares the layer's lists and needs no
        // standalone mode of its own.
        if let Some(layer) = &self.layer {
            for map in &layer.maps {
                let mut one = self.for_map(&map.slice);
                one.standalone = one.standalone || map.slice.is_interior();
                one.check_mode()?;
            }
            return Ok(());
        }
        match &self.slice.interior {
            Some(id) if !self.standalone && !self.slice.wants_layer() => Err(ConvertError::Slice {
                name: self.slice.name.clone(),
                reason: format!(
                    "interior {id} builds a standalone demo map (its own walls and tiles \
                     lists); pass --standalone"
                ),
            }),
            _ => Ok(()),
        }
    }

    pub fn mir_dir(&self) -> PathBuf {
        self.out.join("mir")
    }
    pub fn sprites_dir(&self) -> PathBuf {
        self.out.join("sprites")
    }
    pub fn critters_dir(&self) -> PathBuf {
        self.out.join("critters")
    }
    pub fn gallery_dir(&self) -> PathBuf {
        self.out.join("gallery")
    }
    pub fn mod_dir(&self) -> PathBuf {
        self.out.join("mods").join("mojave")
    }

    /// The directory of the map stage's renders and grounds
    /// (`crate::cache::Store`): `--cache-dir`, else `<out>/cache`.
    pub fn store_dir(&self) -> PathBuf {
        self.cache_dir
            .clone()
            .unwrap_or_else(|| self.out.join("cache"))
    }

    /// Reads and parses the Fallout 2 palette.
    pub fn palette(&self) -> Result<Palette, ConvertError> {
        let f2_error = |reason: String| ConvertError::F2 {
            path: self.f2.clone(),
            reason,
        };
        let bytes = self.f2_file("color.pal")?;
        Palette::parse(&bytes).map_err(|e| f2_error(format!("color.pal: {e}")))
    }

    /// Reads one file of the Fallout 2 install: a loose file under `data/`
    /// wins over the main archive.
    pub fn f2_file(&self, path: &str) -> Result<Vec<u8>, ConvertError> {
        f2_file_in(&self.f2, self.f2_master.as_deref(), path)
    }
}

/// A calibration file's text: the calibration, and its `[sim]` table (the
/// simulation stage's options; the calibration itself refuses unknown
/// keys, so the table is taken out first).
pub fn split_config(
    text: &str,
) -> Result<(foundry_render::Calibration, transpile::ir::SimConfig), String> {
    let mut table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let sim = match table.remove("sim") {
        Some(value) => value
            .try_into::<transpile::ir::SimConfig>()
            .map_err(|e| format!("[sim]: {e}"))?,
        None => transpile::ir::SimConfig::default(),
    };
    let rest = toml::to_string(&table).map_err(|e| e.to_string())?;
    let calibration = foundry_render::Calibration::from_toml(&rest).map_err(|e| e.to_string())?;
    Ok((calibration, sim))
}

/// The main archive of a Fallout 2 install, or `None` when it is loose
/// files only; an error when `f2` is no install.
pub fn f2_master(f2: &Path) -> Result<Option<PathBuf>, ConvertError> {
    find_master(f2)
}

/// Reads one file of the Fallout 2 install at `f2` (main archive `master`,
/// [`f2_master`]): a loose file under `data/` wins over the archives.
pub fn f2_file_in(f2: &Path, master: Option<&Path>, path: &str) -> Result<Vec<u8>, ConvertError> {
    {
        let f2_error = |reason: String| ConvertError::F2 {
            path: f2.to_owned(),
            reason,
        };
        let loose = f2.join("data").join(path.replace('\\', "/"));
        if loose.is_file() {
            return std::fs::read(&loose).map_err(|source| ConvertError::Io {
                path: loose,
                source,
            });
        }
        let Some(master) = master else {
            return Err(f2_error(format!("{path}: not found under data/")));
        };
        // Later archives override earlier ones, as the game mounts them.
        let mut archives = vec![master.to_owned()];
        for name in ["critter.dat", "patch000.dat"] {
            let extra = f2.join(name);
            if extra.is_file() {
                archives.push(extra);
            }
        }
        let mut last_error = format!("{path}: not found");
        for archive_path in archives.iter().rev() {
            let archive = Dat2Archive::open(archive_path)
                .map_err(|e| f2_error(format!("{}: {e}", archive_path.display())))?;
            match archive.read(path) {
                Ok(bytes) => return Ok(bytes),
                Err(error) => last_error = format!("{path}: {error}"),
            }
        }
        Err(f2_error(last_error))
    }
}

/// The main archive, or `None` when the install is loose files only.
fn find_master(f2: &Path) -> Result<Option<PathBuf>, ConvertError> {
    let error = |reason: &str| ConvertError::F2 {
        path: f2.to_owned(),
        reason: reason.to_owned(),
    };
    if !f2.is_dir() {
        return Err(error("not a directory"));
    }
    let master = ["master.dat", "MASTER.DAT", "Master.dat"]
        .iter()
        .map(|name| f2.join(name))
        .find(|path| path.is_file());
    if master.is_none() && !f2.join("data").join("color.pal").is_file() {
        return Err(error("neither master.dat nor data/color.pal is here"));
    }
    Ok(master)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &str = r#"
name = "room"
worldspace = "TestWorld"
plugins = ["Test.esm"]
x = [-1, 1]
y = [-1, 1]
interior = "TestRoomInterior"
"#;

    #[test]
    fn an_interior_slice_parses_with_defaults_and_round_trips() {
        let spec = SliceSpec::from_toml(ROOM).expect("parses");
        assert!(spec.is_interior());
        assert_eq!(spec.interior.as_deref(), Some("TestRoomInterior"));
        assert_eq!(spec.floor_z, None);
        assert!(!spec.place_disabled && !spec.clamp_levels);
        let slicer = spec.interior_slice(100.0);
        assert_eq!(slicer, map_slicer::InteriorSlice::at(100.0));
        let build = spec.interior_build(100.0);
        assert_eq!(build, map_builder::InteriorBuild::new(100.0));
        assert_eq!(SliceSpec::from_toml(&spec.to_toml()), Ok(spec));
    }

    #[test]
    fn interior_settings_reach_the_slicer_and_the_builder() {
        let text = format!(
            "{ROOM}floor_z = 8128.0\nplace_disabled = true\nclamp_levels = true\n\
             cut_height = 120.0\nfloor_band = [-80.0, 24.0]\nhang_clearance = 100.0\n\
             walk_band = [-80.0, 60.0]\nentrance = [10.0, -20.0]\nmap_center = [5.0, 6.0]\n"
        );
        let spec = SliceSpec::from_toml(&text).expect("parses");
        assert_eq!(spec.floor_z, Some(8128.0));
        assert_eq!(spec.entrance, Some([10.0, -20.0]));
        let slicer = spec.interior_slice(8128.0);
        assert_eq!(slicer.cut_height, 120.0);
        assert_eq!(slicer.floor_band, [-80.0, 24.0]);
        assert_eq!(slicer.hang_clearance, 100.0);
        let build = spec.interior_build(8128.0);
        assert_eq!((build.below, build.above), (80.0, 60.0));
        assert!(build.clamp_levels);
        assert_eq!(build.entrance_near, None);
        assert_eq!(SliceSpec::from_toml(&spec.to_toml()), Ok(spec));
    }

    #[test]
    fn an_exterior_slice_writes_no_interior_keys() {
        let spec = SliceSpec::preset("goodsprings").expect("a preset");
        assert!(!spec.is_interior());
        let text = spec.to_toml();
        for key in [
            "interior",
            "floor_z",
            "place_disabled",
            "clamp_levels",
            "cut_height",
            "floor_band",
            "hang_clearance",
            "walk_band",
            "entrance",
        ] {
            assert!(
                !text.lines().any(|l| l.starts_with(&format!("{key} ="))),
                "{key} in {text}"
            );
        }
    }

    #[test]
    fn bad_interior_settings_are_refused() {
        let bad = [
            ("x = [0, 0]\ny = [0, 0]\nfloor_z = 5.0\n", "only apply"),
            (
                "x = [0, 0]\ny = [0, 0]\nplace_disabled = true\n",
                "only apply",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"a room\"\n",
                "editor id",
            ),
            ("x = [0, 0]\ny = [0, 0]\ninterior = \"\"\n", "editor id"),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\ninterior_depth = 0\n",
                "interior_depth",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\nfloor_z = nan\n",
                "floor_z",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\ncut_height = -1.0\n",
                "cut_height",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\ncut_height = 20.0\n",
                "floor_band",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\nfloor_band = [10.0, 0.0]\n",
                "floor_band",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\nwalk_band = [10.0, 40.0]\n",
                "walk_band",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\nhang_clearance = -3.0\n",
                "hang_clearance",
            ),
            (
                "x = [0, 0]\ny = [0, 0]\ninterior = \"R\"\nentrance = [inf, 0.0]\n",
                "entrance",
            ),
        ];
        for (body, expected) in bad {
            let text = format!("name = \"r\"\nworldspace = \"W\"\nplugins = [\"T.esm\"]\n{body}");
            let error = SliceSpec::from_toml(&text).expect_err(body);
            assert!(error.contains(expected), "{body}: {error}");
        }
    }

    #[test]
    fn a_slice_states_its_game_or_its_plugins_do() {
        let base = "name = \"t\"\nworldspace = \"Wasteland\"\nx = [0, 0]\ny = [0, 0]\n";
        let spec =
            SliceSpec::from_toml(&format!("{base}plugins = [\"Fallout3.esm\"]\n")).expect("parses");
        assert_eq!(spec.game, None);
        assert_eq!(spec.stated_game(), Some(mir::GameVariant::Fallout3));
        for text in ["fo3", "fallout3", "FO3"] {
            let spec = SliceSpec::from_toml(&format!(
                "{base}game = \"{text}\"\nplugins = [\"Fallout3.esm\"]\n"
            ))
            .expect("parses");
            assert_eq!(spec.game, Some(mir::GameVariant::Fallout3));
            // Written back in the short form, and read again.
            assert!(
                spec.to_toml().contains("game = \"fo3\""),
                "{}",
                spec.to_toml()
            );
            assert_eq!(SliceSpec::from_toml(&spec.to_toml()), Ok(spec));
        }
        // A synthetic plugin list says nothing; the key says it.
        let spec = SliceSpec::from_toml(&format!("{base}game = \"fnv\"\nplugins = [\"T.esm\"]\n"))
            .expect("parses");
        assert_eq!(spec.stated_game(), Some(mir::GameVariant::FalloutNewVegas));
        let spec = SliceSpec::from_toml(&format!("{base}plugins = [\"T.esm\"]\n")).expect("parses");
        assert_eq!(spec.stated_game(), None);
        // The key and the master disagree, or the key is no game.
        let error = SliceSpec::from_toml(&format!(
            "{base}game = \"fnv\"\nplugins = [\"Fallout3.esm\"]\n"
        ))
        .expect_err("a mismatch");
        assert!(error.contains("master of Fallout 3"), "{error}");
        let error = SliceSpec::from_toml(&format!("{base}game = \"fo4\"\nplugins = [\"T.esm\"]\n"))
            .expect_err("no game");
        assert!(error.contains("fo4"), "{error}");
    }

    #[test]
    fn an_interior_run_needs_standalone() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut inputs = Inputs {
            data_dir: dir.path().to_owned(),
            game: mir::GameVariant::FalloutNewVegas,
            f2: dir.path().to_owned(),
            f2_master: None,
            out: dir.path().to_owned(),
            slice: SliceSpec::from_toml(ROOM).expect("parses"),
            jobs: 1,
            calibration: foundry_render::Calibration::default(),
            backend: foundry_render::Backend::Cpu,
            outline: false,
            standalone: false,
            declutter: None,
            loot: None,
            rebalance: None,
            slice_file: None,
            critter: CritterSettings::default(),
            layer: None,
            with_interiors: false,
            cache_dir: None,
            sim: transpile::ir::SimConfig::default(),
        };
        let error = inputs.check_mode().expect_err("refused").to_string();
        assert!(error.contains("--standalone"), "{error}");
        inputs.standalone = true;
        assert!(inputs.check_mode().is_ok());
        inputs.slice = SliceSpec::preset("goodsprings").expect("a preset");
        inputs.standalone = false;
        assert!(inputs.check_mode().is_ok());
    }

    #[test]
    fn the_readable_profile_turns_the_figure_look_on_above_minimal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut inputs = Inputs {
            data_dir: dir.path().to_owned(),
            game: mir::GameVariant::Fallout3,
            f2: dir.path().to_owned(),
            f2_master: None,
            out: dir.path().to_owned(),
            slice: SliceSpec::from_toml(ROOM).expect("parses"),
            jobs: 1,
            calibration: foundry_render::Calibration::default(),
            backend: foundry_render::Backend::Cpu,
            outline: false,
            standalone: true,
            declutter: None,
            loot: None,
            rebalance: None,
            slice_file: None,
            critter: CritterSettings::default(),
            layer: None,
            with_interiors: false,
            cache_dir: None,
            sim: transpile::ir::SimConfig::default(),
        };
        let look = |i: &Inputs| i.critter_settings().expect("settings").look;
        // Declutter off: nothing set, at any level.
        inputs.slice.animations = Some("combat".to_owned());
        assert_eq!(look(&inputs), None);
        // Readable from the command line, above minimal: on.
        inputs.declutter = Some("readable".to_owned());
        assert_eq!(look(&inputs), Some(true));
        // The slice says default: off, as the flag would.
        inputs.slice.figure_look = Some("default".to_owned());
        assert_eq!(look(&inputs), Some(false));
        inputs.slice.figure_look = None;
        inputs.critter.look = Some(false);
        assert_eq!(look(&inputs), Some(false));
        inputs.critter.look = None;
        // The minimal level draws figures as the first wave did.
        inputs.slice.animations = None;
        assert_eq!(look(&inputs), None);
        // Readable from the slice's own table.
        inputs.declutter = None;
        inputs.slice.animations = Some("combat".to_owned());
        inputs.slice.declutter = Some(crate::declutter::DeclutterSpec {
            profile: Some("readable".to_owned()),
            ..Default::default()
        });
        assert_eq!(look(&inputs), Some(true));
    }
}
