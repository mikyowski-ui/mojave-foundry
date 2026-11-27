//! The readability dials: opt-in changes to how the map stage lays the
//! source world out and draws it, for a town that reads squished, noisy or
//! disorienting under the fixed camera.
//!
//! **Where they are set.** A `[readability]` table in a slice file; in a
//! layer file at the top level (under every map's own slice keys), in
//! `[all_maps]` and in a `[[maps]]` entry (over them); and the repeatable
//! `--readability KEY=VALUE` on `map`, `all` and `estimate`, which wins over
//! every file. Each key is merged on its own. Unknown keys are refused.
//! [`crate::Inputs::readability`] gives a run's (or, through
//! `Inputs::for_map`, one map's) effective [`Readability`]; the map stage
//! keeps it on its scene (`map::Scene::readability`, and the layer's
//! `MapRun::readability`).
//!
//! **Defaults are today's behaviour.** With every field at its default the
//! stage's transforms are identities, nothing is turned or scaled, the cache
//! keys are those of a run without the table (the table is hashed only when
//! some field differs, [`Readability::key_text`]) and every output byte is
//! what it was.
//!
//! **The dials built here** (the others are parsed and carried for the
//! passes that use them):
//!
//! - `world_yaw_deg` (default 0): the source world turned clockwise, seen
//!   from above, about the map window's centre (an interior's `map_center`),
//!   before anything else: `mir::WorldYaw`, which every consumer of source
//!   positions and orientations goes through (the map stage turns the cells'
//!   references and navmeshes, the teleports into each map with that map's
//!   turn, the slice's `entrance`; the ground sampler and the terrain bake
//!   read the land through the inverse). Quarter turns are exact.
//! - `units_per_hex` (default 70): source units per hex along the ground.
//!   Today's value is the calibration's: one floor square is
//!   `units_per_metre · tile_metres` = 70 · 2 = 140 units, and a square is
//!   two hexes along either ground axis, so 70 units per hex. Any other value
//!   sets the map stage's floor square to twice it (the projection's
//!   `tile_metres`); heights keep `pixels_per_metre_vertical`, so the
//!   ground, the terrain, the statics' footprints and their drawn width and
//!   depth, the navmesh, links, markers and triggers spread out together
//!   while the storey stays 96 pixels. Critters keep the critter stage's
//!   scale, and items are drawn at their size ([`Readability::item_scale`]).
//! - `wall_z_scale` (default 1): the drawn height of walls and building
//!   shells (`map_slicer::SlicerConfig::wall_z_scale`).

use serde::{Deserialize, Serialize};

/// The `[readability]` table as written: every key optional, so tables
/// laid over one another merge key by key ([`ReadabilitySpec::over`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadabilitySpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_yaw_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units_per_hex: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_z_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_level_dim: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_level_desaturate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_kerb_narrow: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrow_width_hexes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_calm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_links: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_cue_strength: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_level_reach: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_level_tiles: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit_view_border: Option<bool>,
}

impl ReadabilitySpec {
    /// `self` with every key `over` sets replaced by `over`'s.
    pub fn over(&self, over: &ReadabilitySpec) -> ReadabilitySpec {
        ReadabilitySpec {
            world_yaw_deg: over.world_yaw_deg.or(self.world_yaw_deg),
            units_per_hex: over.units_per_hex.or(self.units_per_hex),
            wall_z_scale: over.wall_z_scale.or(self.wall_z_scale),
            lower_level_dim: over.lower_level_dim.or(self.lower_level_dim),
            lower_level_desaturate: over.lower_level_desaturate.or(self.lower_level_desaturate),
            foreground_kerb_narrow: over.foreground_kerb_narrow.or(self.foreground_kerb_narrow),
            narrow_width_hexes: over.narrow_width_hexes.or(self.narrow_width_hexes),
            ground_calm: over.ground_calm.or(self.ground_calm),
            mark_links: over.mark_links.or(self.mark_links),
            link_cue_strength: over.link_cue_strength.or(self.link_cue_strength),
            lower_level_reach: over.lower_level_reach.or(self.lower_level_reach),
            lower_level_tiles: over.lower_level_tiles.or(self.lower_level_tiles),
            fit_view_border: over.fit_view_border.or(self.fit_view_border),
        }
    }

    /// Whether no key is set.
    pub fn is_empty(&self) -> bool {
        *self == ReadabilitySpec::default()
    }

    /// One `--readability KEY=VALUE` argument: the value is read as TOML
    /// (`90`, `0.78`, `true`), an integer accepted where a number is meant.
    pub fn parse_assignment(text: &str) -> Result<ReadabilitySpec, String> {
        let Some((key, value)) = text.split_once('=') else {
            return Err(format!("--readability {text}: expected KEY=VALUE"));
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
            return Err(format!("--readability {text}: {key:?} is not a key"));
        }
        let float_keys = [
            "world_yaw_deg",
            "units_per_hex",
            "wall_z_scale",
            "lower_level_dim",
            "lower_level_desaturate",
            "foreground_kerb_narrow",
            "ground_calm",
            "link_cue_strength",
        ];
        // `90` is an integer in TOML; these keys are numbers.
        let value = if float_keys.contains(&key)
            && !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-' || b == b'+')
        {
            format!("{value}.0")
        } else {
            value.to_owned()
        };
        toml::from_str::<ReadabilitySpec>(&format!("{key} = {value}"))
            .map_err(|e| format!("--readability {text}: {}", e.message()))
    }
}

/// The effective dials of one map (module notes). Field names are the
/// table's keys.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Readability {
    /// The source world's turn, degrees clockwise seen from above, about the
    /// map window's centre (an interior's `map_center`). 0: no turn.
    pub world_yaw_deg: f64,
    /// Source units per hex along the ground; 70 keeps the calibration.
    pub units_per_hex: f64,
    /// The drawn height of walls and building shells; 1: as before.
    pub wall_z_scale: f64,
    /// The lower-level context (`crate::context`): on an elevation above
    /// the ground (an exterior level, an interior storey), the blank floor
    /// near its own walkable hexes shows the elevation below as seen from
    /// the camera, at this brightness (0.35: 35 % of its own). 0: off, the
    /// blank floor stays black as before.
    pub lower_level_dim: f64,
    /// How much the context loses its colour, 0 (none) to 1 (grey).
    pub lower_level_desaturate: f64,
    /// Context is drawn only on floor squares within this many hex steps of
    /// a walkable hex of the elevation (default 12); farther stays black.
    pub lower_level_reach: u32,
    /// The floor tiles one elevation's context may take from the layer's
    /// tile list: one for every four squares of context, at most this many
    /// (default 256). They are kept back from the terrain's shares, and the
    /// elevation's floor and context are merged into its share plus them.
    pub lower_level_tiles: u32,
    /// For the cutaway (slicer, `declutter`): the narrowed kerb's height in
    /// the foreground band; `None`: the kerb as before.
    pub foreground_kerb_narrow: Option<f64>,
    /// For the cutaway: how many hexes wide the foreground band is.
    pub narrow_width_hexes: u32,
    /// For the tone pass: how much the ground's texture contrast is calmed,
    /// 0 to 1. 0: not at all.
    pub ground_calm: f64,
    /// For the builder's marks: draw a mark on every link's exit grid.
    pub mark_links: bool,
    /// How strongly a doorway's cue frame is blended in, 0 to 1
    /// (`map_builder::marks::LinkCues::strength`, default 0.45).
    pub link_cue_strength: f64,
    /// Place the map window so that the walkable extent (the navmesh of
    /// every elevation) is centred inside the engine's view border
    /// (`crate::view_fit`) instead of centring the slice's window (exteriors;
    /// an interior is centred on its navmesh already). false:
    /// today's placement.
    pub fit_view_border: bool,
}

/// The default of [`Readability::lower_level_reach`].
pub const DEFAULT_LOWER_LEVEL_REACH: u32 = 12;
/// The default of [`Readability::lower_level_tiles`].
pub const DEFAULT_LOWER_LEVEL_TILES: u32 = 256;

/// Today's units per hex: a 140-unit floor square, two hexes across.
pub const DEFAULT_UNITS_PER_HEX: f64 = 70.0;

impl Default for Readability {
    fn default() -> Self {
        Readability {
            world_yaw_deg: 0.0,
            units_per_hex: DEFAULT_UNITS_PER_HEX,
            wall_z_scale: 1.0,
            lower_level_dim: 0.0,
            lower_level_desaturate: 0.0,
            foreground_kerb_narrow: None,
            narrow_width_hexes: 3,
            ground_calm: 0.0,
            mark_links: false,
            link_cue_strength: map_builder::marks::DEFAULT_STRENGTH,
            lower_level_reach: DEFAULT_LOWER_LEVEL_REACH,
            lower_level_tiles: DEFAULT_LOWER_LEVEL_TILES,
            fit_view_border: false,
        }
    }
}

impl Readability {
    /// The defaults with the table's keys laid over them, validated.
    pub fn from_spec(spec: &ReadabilitySpec) -> Result<Self, String> {
        let d = Readability::default();
        let r = Readability {
            world_yaw_deg: spec.world_yaw_deg.unwrap_or(d.world_yaw_deg),
            units_per_hex: spec.units_per_hex.unwrap_or(d.units_per_hex),
            wall_z_scale: spec.wall_z_scale.unwrap_or(d.wall_z_scale),
            lower_level_dim: spec.lower_level_dim.unwrap_or(d.lower_level_dim),
            lower_level_desaturate: spec
                .lower_level_desaturate
                .unwrap_or(d.lower_level_desaturate),
            foreground_kerb_narrow: spec.foreground_kerb_narrow.or(d.foreground_kerb_narrow),
            narrow_width_hexes: spec.narrow_width_hexes.unwrap_or(d.narrow_width_hexes),
            ground_calm: spec.ground_calm.unwrap_or(d.ground_calm),
            mark_links: spec.mark_links.unwrap_or(d.mark_links),
            link_cue_strength: spec.link_cue_strength.unwrap_or(d.link_cue_strength),
            lower_level_reach: spec.lower_level_reach.unwrap_or(d.lower_level_reach),
            lower_level_tiles: spec.lower_level_tiles.unwrap_or(d.lower_level_tiles),
            fit_view_border: spec.fit_view_border.unwrap_or(d.fit_view_border),
        };
        r.validate()?;
        Ok(r)
    }

    /// Checks every value against its range.
    pub fn validate(&self) -> Result<(), String> {
        let bad = |key: &str, value: f64, range: &str| {
            Err(format!("[readability] {key} = {value}: expected {range}"))
        };
        if !self.world_yaw_deg.is_finite() {
            return bad("world_yaw_deg", self.world_yaw_deg, "a finite angle");
        }
        if !(self.units_per_hex.is_finite() && (20.0..=280.0).contains(&self.units_per_hex)) {
            return bad("units_per_hex", self.units_per_hex, "20 to 280");
        }
        if !(self.wall_z_scale.is_finite() && (0.25..=2.0).contains(&self.wall_z_scale)) {
            return bad("wall_z_scale", self.wall_z_scale, "0.25 to 2");
        }
        for (key, value) in [
            ("lower_level_dim", self.lower_level_dim),
            ("lower_level_desaturate", self.lower_level_desaturate),
            ("ground_calm", self.ground_calm),
            ("link_cue_strength", self.link_cue_strength),
        ] {
            if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
                return bad(key, value, "0 to 1");
            }
        }
        if let Some(v) = self.foreground_kerb_narrow
            && !(v.is_finite() && v >= 0.0)
        {
            return bad("foreground_kerb_narrow", v, "a height of 0 or more");
        }
        if !(1..=100).contains(&self.lower_level_reach) {
            return Err(format!(
                "[readability] lower_level_reach = {}: expected 1 to 100",
                self.lower_level_reach
            ));
        }
        if !(16..=2048).contains(&self.lower_level_tiles) {
            return Err(format!(
                "[readability] lower_level_tiles = {}: expected 16 to 2048",
                self.lower_level_tiles
            ));
        }
        if !(1..=64).contains(&self.narrow_width_hexes) {
            return Err(format!(
                "[readability] narrow_width_hexes = {}: expected 1 to 64",
                self.narrow_width_hexes
            ));
        }
        Ok(())
    }

    /// Whether every field is at its default.
    pub fn is_default(&self) -> bool {
        *self == Readability::default()
    }

    /// The text hashed into the map stage's key when a field differs from
    /// its default: every field, in a fixed order.
    pub fn key_text(&self) -> String {
        format!("readability-1 {self:?}")
    }

    /// The turn of the world for a map whose window is centred on `centre`:
    /// `world_yaw_deg`, less 90° for each of the calibration's
    /// `yaw_quarter_turns` (counter-clockwise quarter turns of the window,
    /// which the map stage never applied: folded in here, about the centre,
    /// and the map stage's projection is built with none).
    pub fn yaw(
        &self,
        centre: [f32; 2],
        calibration: &foundry_render::Calibration,
    ) -> mir::WorldYaw {
        let quarters = f64::from(calibration.yaw_quarter_turns.rem_euclid(4));
        mir::WorldYaw::new(centre, self.world_yaw_deg - 90.0 * quarters)
    }

    /// The calibration the map stage projects with: `calibration` itself
    /// when `units_per_hex` is at its default and there is no quarter turn;
    /// otherwise its floor square set to `2 · units_per_hex` units (through
    /// `tile_metres`, so heights keep their scale) and its quarter turns
    /// cleared ([`Readability::yaw`] carries them).
    pub fn map_calibration(
        &self,
        calibration: &foundry_render::Calibration,
    ) -> foundry_render::Calibration {
        let mut out = calibration.clone();
        if self.units_per_hex != DEFAULT_UNITS_PER_HEX {
            out.tile_metres =
                (2.0 * self.units_per_hex / f64::from(calibration.units_per_metre)) as f32;
        }
        out.yaw_quarter_turns = 0;
        out
    }

    /// How much wider the ground is drawn than the calibration draws it:
    /// the calibration's floor square over the map's (1.25 at 56 units per
    /// hex).
    pub fn spread(&self, calibration: &foundry_render::Calibration) -> f64 {
        f64::from(calibration.tile_units())
            / f64::from(self.map_calibration(calibration).tile_units())
    }

    /// The horizontal scale an item sprite is drawn with so that it keeps
    /// the size the calibration gives it on a spread-out map: `1 / spread`.
    pub fn item_scale(&self, calibration: &foundry_render::Calibration) -> f32 {
        if self.units_per_hex == DEFAULT_UNITS_PER_HEX {
            1.0
        } else {
            (1.0 / self.spread(calibration)) as f32
        }
    }

    /// The report's lines (`## Readability dials`).
    pub fn lines(&self, calibration: &foundry_render::Calibration) -> Vec<String> {
        let map = self.map_calibration(calibration);
        let mut lines = vec![
            format!(
                "world_yaw_deg = {} (clockwise seen from above, about the window's centre{})",
                self.world_yaw_deg,
                if calibration.yaw_quarter_turns.rem_euclid(4) != 0 {
                    format!(
                        "; the calibration's yaw_quarter_turns = {} folded in",
                        calibration.yaw_quarter_turns
                    )
                } else {
                    String::new()
                }
            ),
            format!(
                "units_per_hex = {}: floor square {} units (calibration {}), ground drawn {:.3} times as wide; storey {} pixels unchanged; items keep their size (drawn at {:.3} horizontally), critters keep the critter stage's scale",
                self.units_per_hex,
                map.tile_units(),
                calibration.tile_units(),
                self.spread(calibration),
                96,
                self.item_scale(calibration)
            ),
            format!(
                "wall_z_scale = {}: walls and building shells (sliced STAT and SCOL) drawn at this height about their anchor; landmarks, doors, items, sprites, relief and critters unchanged",
                self.wall_z_scale
            ),
        ];
        if self.lower_level_dim != 0.0 {
            lines.push(format!(
                "lower_level_dim = {}, lower_level_desaturate = {}, lower_level_reach = {}, lower_level_tiles = {}: an elevation above the ground shows the one below as dimmed floor context within the reach of its walkable hexes, merged into at most that many tiles (`## FID budget`)",
                self.lower_level_dim,
                self.lower_level_desaturate,
                self.lower_level_reach,
                self.lower_level_tiles
            ));
        }
        if let Some(kerb) = self.foreground_kerb_narrow {
            lines.push(format!(
                "foreground_kerb_narrow = {kerb}, narrow_width_hexes = {}: cut strips beside a walkway at most that wide are cut to this kerb; outdoors, without cutaway_exteriors, only those strips are cut (`## Declutter`)",
                self.narrow_width_hexes
            ));
        }
        if self.ground_calm != 0.0 {
            lines.push(format!(
                "ground_calm = {}: the floor tone calmed, and the terrain with it",
                self.ground_calm
            ));
        }
        if self.mark_links {
            lines.push(format!(
                "mark_links = true, link_cue_strength = {}: cues painted into the floor under every link (the notes give the counts)",
                self.link_cue_strength
            ));
        }
        lines.push(
            "with a dial set, a stair whose landing is a dead-end pocket (at most 60 walkable hexes, no other stair or door) is dropped (`map_builder::LayerOptions::prune_dead_end_stairs`; the notes name each)"
                .to_owned(),
        );
        if self.fit_view_border {
            lines.push(
                "fit_view_border = true: an exterior window placed so that the walkable extent is centred in the engine's view border (the view border lines below; an interior is centred on its navmesh already)"
                    .to_owned(),
            );
        }
        lines
    }
}

/// The source window that a map window turned by `yaw` reads: the bounding
/// square of its corners turned back to source space. The window itself
/// when there is no turn (and so for every quarter turn about its centre,
/// up to rounding: the identity is returned exactly).
pub fn source_window(window: &mir::MapWindow, yaw: &mir::WorldYaw) -> mir::MapWindow {
    // A quarter turn about the window's own centre maps it onto itself; a
    // window placed elsewhere (`fit_view_border`) is turned back.
    let centred = {
        let c = yaw.centre();
        let mid = [
            f64::from(window.x_west + window.size * 0.5),
            f64::from(window.y_north - window.size * 0.5),
        ];
        (mid[0] - c[0]).abs() < 0.5 && (mid[1] - c[1]).abs() < 0.5
    };
    if yaw.is_identity() || (yaw.quarter_turns().is_some() && centred) {
        return *window;
    }
    let back = yaw.inverse();
    let corners = [
        [window.x_west, window.y_north],
        [window.x_west + window.size, window.y_north],
        [window.x_west, window.y_north - window.size],
        [window.x_west + window.size, window.y_north - window.size],
    ];
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for [x, y] in corners {
        let p = back.apply_xy(x, y);
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    mir::MapWindow {
        x_west: lo[0],
        y_north: hi[1],
        size,
    }
}

/// Turns every teleport of `cells` and `references` that leads into a map
/// with a turn: `turn_of` gives the turn of the map built from a cell (by
/// the cell's form key), `None` for a cell no turned map is built from (a
/// stub's interior, another worldspace). A teleport's arrival lies in the
/// space of the map it leads to, so it takes that map's turn, not the turn
/// of the map its door stands on. Returns how many were turned.
pub fn turn_teleports(
    cells: &mut [mir::CellDoc],
    references: &mut [mir::Reference],
    turn_of: &dyn Fn(&mir::FormKey) -> Option<mir::WorldYaw>,
) -> usize {
    let mut turned = 0;
    let mut turn = |r: &mut mir::Reference| {
        if let Some(teleport) = &mut r.teleport
            && let Some(cell) = &teleport.cell
            && let Some(yaw) = turn_of(&cell.form)
            && !yaw.is_identity()
        {
            yaw.turn_teleport(teleport);
            turned += 1;
        }
    };
    for cell in cells.iter_mut() {
        for r in &mut cell.references {
            turn(r);
        }
    }
    for r in references.iter_mut() {
        turn(r);
    }
    turned
}

/// What a turned or spread-out window leaves off: the references of the
/// scene's cells that today's window (the calibration's, not turned) holds
/// and the map's does not.
#[derive(Debug, Clone, Default)]
pub struct Clip {
    /// Whether the map's window differs from today's at all.
    pub active: bool,
    /// Today's window side and the map's, source units.
    pub today_side: f32,
    pub map_side: f32,
    /// The references left off (turned, as the map sees them), sorted by
    /// form key.
    pub references: Vec<mir::Reference>,
    /// Their source bounding box: `[west, south]`, `[east, north]`.
    pub extent: Option<[[f32; 2]; 2]>,
    /// Navmesh triangles of the cells whose centroid today's window holds
    /// and the map's does not, of all such triangles today's holds.
    pub triangles_left: usize,
    pub triangles_today: usize,
}

impl Clip {
    /// Compares the source `cells` (before the turn) under today's window
    /// with the map's window under `yaw`.
    pub fn of(
        cells: &[mir::CellDoc],
        today: &mir::MapWindow,
        window: &mir::MapWindow,
        yaw: &mir::WorldYaw,
    ) -> Clip {
        let active = !yaw.is_identity() || today != window;
        let mut clip = Clip {
            active,
            today_side: today.size,
            map_side: window.size,
            ..Clip::default()
        };
        if !active {
            return clip;
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for cell in cells {
            for r in &cell.references {
                let [x, y, _] = r.transform.position;
                if r.flags.deleted || !r.transform.is_finite() || !today.contains(x, y) {
                    continue;
                }
                let [mx, my] = yaw.apply_xy(x, y);
                if window.contains(mx, my) {
                    continue;
                }
                let mut turned = r.clone();
                yaw.turn_reference(&mut turned);
                clip.references.push(turned);
                lo = [lo[0].min(x), lo[1].min(y)];
                hi = [hi[0].max(x), hi[1].max(y)];
            }
            for navmesh in &cell.navmeshes {
                for t in 0..navmesh.triangles.len() {
                    let Some(c) = navmesh.corners(t) else {
                        continue;
                    };
                    let x = (c[0][0] + c[1][0] + c[2][0]) / 3.0;
                    let y = (c[0][1] + c[1][1] + c[2][1]) / 3.0;
                    if !today.contains(x, y) {
                        continue;
                    }
                    clip.triangles_today += 1;
                    let [mx, my] = yaw.apply_xy(x, y);
                    if !window.contains(mx, my) {
                        clip.triangles_left += 1;
                    }
                }
            }
        }
        clip.references.sort_by_cached_key(|r| r.form.to_string());
        if lo[0] <= hi[0] {
            clip.extent = Some([lo, hi]);
        }
        clip
    }

    /// The teleport doors left off.
    pub fn doors(&self) -> impl Iterator<Item = &mir::Reference> {
        self.references.iter().filter(|r| r.teleport.is_some())
    }

    /// The report's lines (`## Readability dials`).
    pub fn lines(&self) -> Vec<String> {
        if !self.active {
            return vec!["window: today's; nothing clipped".to_owned()];
        }
        let mut lines = vec![format!(
            "window: {} units square (today's {}); of today's window, {} references and {} of {} navmesh triangles fall outside the map",
            self.map_side,
            self.today_side,
            self.references.len(),
            self.triangles_left,
            self.triangles_today
        )];
        if let Some([lo, hi]) = self.extent {
            lines.push(format!(
                "clipped extent (source): x {:.0} to {:.0}, y {:.0} to {:.0}",
                lo[0], hi[0], lo[1], hi[1]
            ));
        }
        let mut kinds: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for r in &self.references {
            *kinds
                .entry(r.base_kind.as_deref().unwrap_or("?"))
                .or_default() += 1;
        }
        if !kinds.is_empty() {
            let text: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
            lines.push(format!("clipped by kind: {}", text.join(", ")));
        }
        for door in self.doors() {
            lines.push(format!(
                "clipped door {}{} (leads to {})",
                door.form,
                door.editor_id
                    .as_deref()
                    .map(|e| format!(" `{e}`"))
                    .unwrap_or_default(),
                door.teleport
                    .as_ref()
                    .and_then(|t| t.cell.as_ref())
                    .map(|c| c.editor_id.clone().unwrap_or_else(|| c.form.to_string()))
                    .unwrap_or_else(|| "nowhere known".to_owned())
            ));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_today() {
        let r = Readability::from_spec(&ReadabilitySpec::default()).unwrap();
        assert!(r.is_default());
        let c = foundry_render::Calibration::default();
        assert_eq!(r.map_calibration(&c), c);
        assert_eq!(r.item_scale(&c), 1.0);
        assert!(r.yaw([1.0, 2.0], &c).is_identity());
        assert_eq!(r.spread(&c), 1.0);
    }

    #[test]
    fn keys_merge_one_by_one_and_unknown_ones_are_refused() {
        let file: ReadabilitySpec =
            toml::from_str("world_yaw_deg = 90.0\nwall_z_scale = 0.8").unwrap();
        let cli = ReadabilitySpec::parse_assignment("world_yaw_deg=180").unwrap();
        let both = file.over(&cli);
        assert_eq!(both.world_yaw_deg, Some(180.0));
        assert_eq!(both.wall_z_scale, Some(0.8));
        assert!(toml::from_str::<ReadabilitySpec>("world_yaw = 1.0").is_err());
        assert!(ReadabilitySpec::parse_assignment("nonsense=1").is_err());
        assert!(ReadabilitySpec::parse_assignment("units_per_hex").is_err());
        assert_eq!(
            ReadabilitySpec::parse_assignment("mark_links=true")
                .unwrap()
                .mark_links,
            Some(true)
        );
        assert_eq!(
            ReadabilitySpec::parse_assignment("narrow_width_hexes=5")
                .unwrap()
                .narrow_width_hexes,
            Some(5)
        );
        let spread = Readability::from_spec(&ReadabilitySpec {
            units_per_hex: Some(56.0),
            ..ReadabilitySpec::default()
        })
        .unwrap();
        let c = foundry_render::Calibration::default();
        assert!((spread.map_calibration(&c).tile_units() - 112.0).abs() < 1e-3);
        assert!((spread.spread(&c) - 1.25).abs() < 1e-5);
        assert!((spread.item_scale(&c) - 0.8).abs() < 1e-5);
        assert!(
            Readability::from_spec(&ReadabilitySpec {
                wall_z_scale: Some(0.0),
                ..ReadabilitySpec::default()
            })
            .is_err()
        );
    }

    #[test]
    fn quarter_turns_of_the_calibration_fold_into_the_yaw() {
        let c = foundry_render::Calibration {
            yaw_quarter_turns: 1,
            ..foundry_render::Calibration::default()
        };
        let r = Readability::default();
        assert_eq!(r.yaw([0.0; 2], &c).degrees(), 270.0);
        assert_eq!(r.map_calibration(&c).yaw_quarter_turns, 0);
    }
}
