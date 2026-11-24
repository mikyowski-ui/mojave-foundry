//! The placement index: `map-placements.json`, written beside
//! `map-report.md` (not inside the layer), the one file the simulation
//! stage needs to find, on the written maps, the references the source
//! plugins talk about (`docs/simulation-layer-spec.md`, section 10.4).
//!
//! # Schema (version 1)
//!
//! ```text
//! { schema_version, origin ("stage" | "reconstructed"), layer, game,
//!   maps:     [ { stem, file, number } ],
//!   placed:   [ PlacedRef ],
//!   unplaced: [ UnplacedRef ],
//!   notes:    [ text ] }
//! ```
//!
//! A [`PlacedRef`] is one reference that became an object of a written
//! map: its `map` (stem), `map_number`, `elevation`, `hex`, `object_id`
//! and `pid` as the map file has them; `kind` and `critter`; and, when the
//! source is known, the `reference` (form key), `editor_id`, `base`,
//! `base_kind` (record signature), `base_editor_id`, `enable_parent`
//! (`reference`, `opposite`), `initially_disabled`, `hidden` (the written
//! object carries the hidden flag) and, for critters, `team` and
//! `ai_packet`. A field the writer cannot know is left out of the JSON.
//!
//! An [`UnplacedRef`] is a reference of a map's cells that is not on the
//! map, with the `reason` (`disabled`, `deleted`, `skipped`, `not_placed`)
//! and an optional `detail`: enable-parent state needs to know them too.
//!
//! Both lists hold the references the simulation can use: critters, doors,
//! containers, items and terminals, and any other reference that is an
//! enable parent or child, or that the caller names (referenced by a
//! script, a quest or a package). Plain scenery is not listed. Both are
//! sorted (map, elevation, object id; map, reference), and nothing in the
//! file depends on the time, the host or a path, so two builds from the
//! same inputs are byte-identical.
//!
//! # Two ways to make it
//!
//! - [`write_for_stage`] is the map stage's call: it joins the stage's own
//!   references, placements and critters with the objects read back from
//!   the written maps (`build`, a pure function of plain inputs).
//! - [`reconstruct`] makes what it can from the written maps alone, for a
//!   run whose map stage did not write the file: no references, bases or
//!   enable parents, only positions, PIDs, object ids and the kinds the
//!   maps show (people get their reference back from `features/<stem>.json`
//!   where that file lists them).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vault_formats::GameProfile;
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::mod_manifest::{MANIFEST, ModManifest};
use vault_formats::pro::BasicFlags;
use vault_formats::pro_catalog::LocalPrototypeCatalog;
use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};

/// The file's name, under the run's output directory.
pub const FILE_NAME: &str = "map-placements.json";

/// The schema version [`PlacementIndex::schema_version`] carries.
pub const SCHEMA_VERSION: u32 = 1;

/// A failure reading or writing the index or the maps it is made from.
#[derive(Debug, thiserror::Error)]
#[error("{what}: {message}")]
pub struct PlacementError {
    /// The file or step.
    pub what: String,
    /// What went wrong.
    pub message: String,
}

fn fail(what: impl Into<String>, message: impl ToString) -> PlacementError {
    PlacementError {
        what: what.into(),
        message: message.to_string(),
    }
}

/// What a placed reference is, for the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacedKind {
    /// An NPC or a creature.
    Critter,
    /// A door.
    Door,
    /// A container.
    Container,
    /// An item lying on the ground.
    Item,
    /// A terminal.
    Terminal,
    /// Anything else (listed only when it is an enable parent or child, or
    /// the caller names it).
    Scenery,
}

impl PlacedKind {
    /// Every kind, in report order.
    pub const ALL: [PlacedKind; 6] = [
        PlacedKind::Critter,
        PlacedKind::Door,
        PlacedKind::Container,
        PlacedKind::Item,
        PlacedKind::Terminal,
        PlacedKind::Scenery,
    ];

    /// The word used in reports and in the JSON.
    pub fn label(self) -> &'static str {
        match self {
            PlacedKind::Critter => "critter",
            PlacedKind::Door => "door",
            PlacedKind::Container => "container",
            PlacedKind::Item => "item",
            PlacedKind::Terminal => "terminal",
            PlacedKind::Scenery => "scenery",
        }
    }

    /// The kind a base record signature names; `None` for a signature the
    /// simulation has no special use for (statics, activators, lights).
    pub fn from_signature(signature: &str) -> Option<PlacedKind> {
        match signature {
            "NPC_" | "CREA" => Some(PlacedKind::Critter),
            "DOOR" => Some(PlacedKind::Door),
            "CONT" => Some(PlacedKind::Container),
            "TERM" => Some(PlacedKind::Terminal),
            "WEAP" | "ARMO" | "AMMO" | "MISC" | "ALCH" | "BOOK" | "KEYM" | "NOTE" | "CLOT"
            | "INGR" | "CHIP" | "CMNY" | "CCRD" | "CDCK" | "IMOD" | "RCPE" => {
                Some(PlacedKind::Item)
            }
            _ => None,
        }
    }
}

/// Where the index came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Written by the map stage, from its own records.
    Stage,
    /// Made from the written maps alone ([`reconstruct`]).
    Reconstructed,
}

/// One map of the layer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MapEntry {
    /// The file stem (`sdmart` for `maps/sdmart.map`).
    pub stem: String,
    /// `maps/<stem>.map`.
    pub file: String,
    /// The map's number in the layer's manifest.
    pub number: u32,
}

/// An enable parent, as the source states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnableParent {
    /// The parent reference's form key.
    pub reference: String,
    /// The child is enabled when the parent is disabled.
    pub opposite: bool,
}

fn one() -> u32 {
    1
}

fn is_one(value: &u32) -> bool {
    *value == 1
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A reference that became an object of a written map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedRef {
    /// The map's file stem.
    pub map: String,
    /// The map's number.
    pub map_number: u32,
    /// The elevation, 0 to 2.
    pub elevation: u8,
    /// The hex, 0 to 39,999.
    pub hex: i32,
    /// The object's id in the written map; absent when the join found no
    /// object (the note says how many).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<u32>,
    /// The object's PID, from the same object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// What it is.
    pub kind: PlacedKind,
    /// Whether it is a critter.
    pub critter: bool,
    /// How many objects the reference became (a landmark is several
    /// pieces, ammunition several packs); the entry describes the first.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub pieces: u32,
    /// The reference's form key, as the MIR names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The reference's editor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    /// The base record's form key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The base record's signature (`DOOR`, `NPC_`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_kind: Option<String>,
    /// The base record's editor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_editor_id: Option<String>,
    /// The enable parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_parent: Option<EnableParent>,
    /// The source starts the reference disabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub initially_disabled: bool,
    /// The written object carries the hidden flag.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// A critter's team in the written map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    /// A critter's AI packet number in the written map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_packet: Option<i32>,
}

/// Why a reference of a map's cells is not on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnplacedReason {
    /// It starts disabled and the stage left it out.
    Disabled,
    /// The winning plugin deleted it.
    Deleted,
    /// The stage names a reason (a critter without art, say): see `detail`.
    Skipped,
    /// It is in the map's cells and nowhere on the map: pruned by the
    /// declutter pass, off the level, outside the window's budget, or not
    /// a thing the stage places (items with loot off).
    NotPlaced,
}

impl UnplacedReason {
    /// Every reason, in report order.
    pub const ALL: [UnplacedReason; 4] = [
        UnplacedReason::Disabled,
        UnplacedReason::Deleted,
        UnplacedReason::Skipped,
        UnplacedReason::NotPlaced,
    ];

    /// The word used in reports and in the JSON.
    pub fn label(self) -> &'static str {
        match self {
            UnplacedReason::Disabled => "disabled",
            UnplacedReason::Deleted => "deleted",
            UnplacedReason::Skipped => "skipped",
            UnplacedReason::NotPlaced => "not_placed",
        }
    }
}

/// A reference of a map's cells that is not on the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnplacedRef {
    /// The map whose cells hold it (file stem).
    pub map: String,
    /// The reference's form key.
    pub reference: String,
    /// What it is.
    pub kind: PlacedKind,
    /// The reference's editor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    /// The base record's form key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The base record's signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_kind: Option<String>,
    /// The base record's editor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_editor_id: Option<String>,
    /// The enable parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_parent: Option<EnableParent>,
    /// The source starts the reference disabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub initially_disabled: bool,
    /// Why.
    pub reason: UnplacedReason,
    /// The stage's own words, when it gave a reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementIndex {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Who made it.
    pub origin: Origin,
    /// The layer's name, from its manifest.
    pub layer: String,
    /// The source game, as `mir::GameVariant::id` names it (`fallout3`).
    pub game: String,
    /// The layer's maps, by number.
    pub maps: Vec<MapEntry>,
    /// The references on the maps, by map, elevation and object id.
    pub placed: Vec<PlacedRef>,
    /// The references of the maps' cells that are not on them.
    pub unplaced: Vec<UnplacedRef>,
    /// Things worth knowing about how it was made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl PlacementIndex {
    /// The file's text: pretty JSON and a final newline.
    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Parses the file's text and checks its schema version.
    pub fn from_text(text: &str) -> Result<Self, PlacementError> {
        let index: PlacementIndex = serde_json::from_str(text).map_err(|e| fail(FILE_NAME, e))?;
        if index.schema_version != SCHEMA_VERSION {
            return Err(fail(
                FILE_NAME,
                format!(
                    "schema version {} where this build reads {SCHEMA_VERSION}; run the map \
                     stage again",
                    index.schema_version
                ),
            ));
        }
        Ok(index)
    }

    /// The map entry for a stem.
    pub fn map(&self, stem: &str) -> Option<&MapEntry> {
        self.maps.iter().find(|m| m.stem == stem)
    }

    fn sort(&mut self) {
        self.maps.sort();
        self.placed.sort_by(|a, b| {
            (
                &a.map,
                a.elevation,
                a.object_id.unwrap_or(u32::MAX),
                a.hex,
                &a.reference,
            )
                .cmp(&(
                    &b.map,
                    b.elevation,
                    b.object_id.unwrap_or(u32::MAX),
                    b.hex,
                    &b.reference,
                ))
        });
        self.unplaced
            .sort_by(|a, b| (&a.map, &a.reference).cmp(&(&b.map, &b.reference)));
    }
}

/// Writes `<out>/map-placements.json`; returns its path.
pub fn write(out: &Path, index: &PlacementIndex) -> Result<PathBuf, PlacementError> {
    let path = out.join(FILE_NAME);
    std::fs::write(&path, index.to_text()).map_err(|e| fail(FILE_NAME, e))?;
    Ok(path)
}

/// Reads `<out>/map-placements.json`; `None` when there is none.
pub fn read(out: &Path) -> Result<Option<PlacementIndex>, PlacementError> {
    let path = out.join(FILE_NAME);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| fail(FILE_NAME, e))?;
    PlacementIndex::from_text(&text).map(Some)
}

// ---- The pure builder ----------------------------------------------------

/// What the join needs to know about a reference of a map's cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    /// The reference's form key.
    pub reference: String,
    /// Its editor id.
    pub editor_id: Option<String>,
    /// The base's form key.
    pub base: Option<String>,
    /// The base's signature.
    pub base_kind: Option<String>,
    /// The base's editor id.
    pub base_editor_id: Option<String>,
    /// An NPC or a creature.
    pub critter: bool,
    /// The enable parent.
    pub enable_parent: Option<EnableParent>,
    /// Starts disabled.
    pub initially_disabled: bool,
    /// Deleted by the winning plugin.
    pub deleted: bool,
}

/// What the stage made of a reference, which decides what object the join
/// looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotClass {
    /// A critter.
    Critter,
    /// A door.
    Door,
    /// A container.
    Container,
    /// An item.
    Item,
    /// Scenery of any kind.
    Scenery,
}

/// One placement the stage made, in the order it made them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    /// The placed reference's form key.
    pub reference: String,
    /// Its elevation.
    pub elevation: u8,
    /// Its hex.
    pub hex: i32,
    /// What the stage made it.
    pub class: SpotClass,
    /// How many objects the builder writes for it: 1, or for an item
    /// the catalogue's count (ammunition in packs; 0 writes none).
    pub objects: u32,
}

/// One top-level object of a written map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenObject {
    /// Its elevation.
    pub elevation: u8,
    /// Its hex.
    pub hex: i32,
    /// The object id in the map file.
    pub id: u32,
    /// Its PID.
    pub pid: u32,
    /// A critter (PID category 1).
    pub critter: bool,
    /// A door.
    pub door: bool,
    /// An exit grid.
    pub exit_grid: bool,
    /// An item-category object with an inventory or room for one.
    pub holds_items: bool,
    /// The hidden flag is set.
    pub hidden: bool,
    /// A critter's team.
    pub team: Option<i32>,
    /// A critter's AI packet.
    pub ai_packet: Option<i32>,
}

/// Everything [`build`] needs about one map.
#[derive(Debug, Clone)]
pub struct MapInput<'a> {
    /// The file stem.
    pub stem: &'a str,
    /// The map's number.
    pub number: u32,
    /// Every reference of the map's cells the stage considered.
    pub references: &'a [SourceRef],
    /// The placements the stage made, in order.
    pub spots: &'a [Spot],
    /// References the stage skipped and its reason for each.
    pub skipped: &'a [(String, String)],
    /// The objects of the written map.
    pub written: &'a [WrittenObject],
}

fn kind_of(base_kind: Option<&str>, critter: bool, class: Option<SpotClass>) -> PlacedKind {
    if critter {
        return PlacedKind::Critter;
    }
    if let Some(kind) = base_kind.and_then(PlacedKind::from_signature) {
        return kind;
    }
    match class {
        Some(SpotClass::Critter) => PlacedKind::Critter,
        Some(SpotClass::Door) => PlacedKind::Door,
        Some(SpotClass::Container) => PlacedKind::Container,
        Some(SpotClass::Item) => PlacedKind::Item,
        Some(SpotClass::Scenery) | None => PlacedKind::Scenery,
    }
}

/// Written objects still unclaimed, by elevation and hex, in file order.
struct Pool<'a> {
    written: &'a [WrittenObject],
    free: BTreeMap<(u8, i32), Vec<usize>>,
}

impl<'a> Pool<'a> {
    fn new(written: &'a [WrittenObject]) -> Self {
        let mut free: BTreeMap<(u8, i32), Vec<usize>> = BTreeMap::new();
        for (i, object) in written.iter().enumerate() {
            free.entry((object.elevation, object.hex))
                .or_default()
                .push(i);
        }
        for list in free.values_mut() {
            list.sort_by_key(|i| written[*i].id);
        }
        Self { written, free }
    }

    /// The first unclaimed object at the spot that fits its class, by its
    /// index in `written`, and the further objects an item became (its
    /// packs: the next unclaimed objects at the hex with its PID). A
    /// placement that finds nothing leaves the pool as it was.
    fn claim(&mut self, spot: &Spot) -> Option<(usize, u32)> {
        if spot.objects == 0 {
            return None;
        }
        let list = self.free.get_mut(&(spot.elevation, spot.hex))?;
        let fits = |o: &WrittenObject, strict: bool| match spot.class {
            SpotClass::Critter => o.critter,
            SpotClass::Door if strict => o.door,
            SpotClass::Door => !o.critter && !o.exit_grid,
            // Items and containers are item prototypes, everything else
            // scenery or walls.
            SpotClass::Item | SpotClass::Container => {
                o.pid >> 24 == 0 && !o.critter && !o.exit_grid && !o.door
            }
            SpotClass::Scenery => o.pid >> 24 != 0 && !o.critter && !o.exit_grid && !o.door,
        };
        let strict = matches!(spot.class, SpotClass::Door);
        let at = list
            .iter()
            .position(|i| fits(&self.written[*i], strict))
            .or_else(|| {
                strict
                    .then(|| list.iter().position(|i| fits(&self.written[*i], false)))
                    .flatten()
            })?;
        let index = list.remove(at);
        Some((index, self.take_packs(spot, index)))
    }

    /// Claims the further objects a spot became after its first (an
    /// item's packs: the next ids at the hex, with its PID); returns how
    /// many it found.
    fn take_packs(&mut self, spot: &Spot, first: usize) -> u32 {
        let object = &self.written[first];
        let Some(list) = self.free.get_mut(&(object.elevation, object.hex)) else {
            return 0;
        };
        let mut packs = 0;
        let mut id = object.id;
        while packs + 1 < spot.objects
            && let Some(at) = list
                .iter()
                .position(|i| self.written[*i].pid == object.pid && self.written[*i].id == id + 1)
        {
            id = self.written[list.remove(at)].id;
            packs += 1;
        }
        packs
    }

    /// For an item or container the builder moved off a link's grid (to
    /// the nearest free walkable hex, where no other placement stands):
    /// the nearest unclaimed item-category object within
    /// [`MOVED_RADIUS`] steps at a hex no placement names, ties to the
    /// lower object id.
    fn claim_moved(&mut self, spot: &Spot, named: &BTreeSet<(u8, i32)>) -> Option<(usize, u32)> {
        if spot.objects == 0 || !matches!(spot.class, SpotClass::Item | SpotClass::Container) {
            return None;
        }
        let written = self.written;
        let best = self
            .free
            .iter()
            .filter(|((e, hex), _)| *e == spot.elevation && !named.contains(&(*e, *hex)))
            .flat_map(|(_, list)| list.iter().copied())
            .filter(|i| {
                let o = &written[*i];
                o.pid >> 24 == 0 && !o.door && !o.exit_grid && !o.critter
            })
            .filter_map(|i| {
                let d =
                    vault_formats::geometry::HexGrid::lattice_distance(spot.hex, written[i].hex)
                        .ok()?;
                (d <= MOVED_RADIUS).then_some((d, written[i].id, i))
            })
            .min()?;
        let (_, _, index) = best;
        let object = &written[index];
        if let Some(list) = self.free.get_mut(&(object.elevation, object.hex)) {
            list.retain(|i| *i != index);
        }
        Some((index, self.take_packs(spot, index)))
    }
}

/// How far (hex steps) the join looks for an item or container the builder
/// moved off a link (`map_builder`'s link clearance moves them to the
/// nearest free walkable hex).
pub const MOVED_RADIUS: i32 = 12;

/// Builds the index from plain inputs.
///
/// `referenced` names references that are listed whatever their kind
/// (those a script, quest or package points at: `mir::Records::referenced_refs`).
///
/// The join of a placement to its object takes the objects of the written
/// map at the placement's elevation and hex in file order, and gives each
/// placement, in the stage's order, the first that fits its class (a
/// critter is a critter, a door a door when one is there, anything else a
/// non-critter, non-door, non-exit-grid). The stage writes objects in
/// placement order, so the n-th placement at a hex gets the n-th object;
/// an item also takes the packs written after it (same hex, same PID,
/// the next ids), counted in `pieces`. The builder moves a ground item or
/// a container that stands on a link's keep-clear hexes to the nearest
/// free walkable hex, where no other placement stands: a placement left
/// without an object then takes the nearest unclaimed item-category object
/// within [`MOVED_RADIUS`] steps at a hex no placement names (counted in
/// the notes), and its `hex` is that object's. Placements that find none
/// are listed without an object id, and counted in the notes.
pub fn build(
    layer: &str,
    game: &str,
    maps: &[MapInput<'_>],
    referenced: &BTreeSet<String>,
) -> PlacementIndex {
    let parents: BTreeSet<&str> = maps
        .iter()
        .flat_map(|m| m.references.iter())
        .filter_map(|r| r.enable_parent.as_ref())
        .map(|p| p.reference.as_str())
        .collect();
    let relevant = |kind: PlacedKind, source: Option<&SourceRef>, reference: &str| {
        kind != PlacedKind::Scenery
            || source.is_some_and(|s| s.enable_parent.is_some())
            || parents.contains(reference)
            || referenced.contains(reference)
    };
    let mut index = PlacementIndex {
        schema_version: SCHEMA_VERSION,
        origin: Origin::Stage,
        layer: layer.to_owned(),
        game: game.to_owned(),
        maps: Vec::new(),
        placed: Vec::new(),
        unplaced: Vec::new(),
        notes: Vec::new(),
    };
    for map in maps {
        index.maps.push(MapEntry {
            stem: map.stem.to_owned(),
            file: format!("maps/{}.map", map.stem),
            number: map.number,
        });
        let sources: BTreeMap<&str, &SourceRef> = map
            .references
            .iter()
            .map(|r| (r.reference.as_str(), r))
            .collect();
        // The first reason given for a reference.
        let mut skipped: BTreeMap<&str, &str> = BTreeMap::new();
        for (r, why) in map.skipped {
            skipped.entry(r.as_str()).or_insert(why.as_str());
        }

        // Every placement claims its object, in order, so that one the
        // index leaves out does not hand its object to a later one; then
        // the items and containers the builder moved off a link take the
        // nearest object left at a hex no placement names.
        let mut pool = Pool::new(map.written);
        let mut claims: Vec<Option<(usize, u32)>> =
            map.spots.iter().map(|spot| pool.claim(spot)).collect();
        let named: BTreeSet<(u8, i32)> = map.spots.iter().map(|s| (s.elevation, s.hex)).collect();
        let mut moved = 0usize;
        for (spot, claim) in map.spots.iter().zip(claims.iter_mut()) {
            if claim.is_none() {
                *claim = pool.claim_moved(spot, &named);
                moved += usize::from(claim.is_some());
            }
        }
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        let mut unmatched = 0usize;
        let mut empty = 0usize;
        for (spot, claim) in map.spots.iter().zip(&claims) {
            let object = claim.map(|(i, _)| &map.written[i]);
            let packs = claim.map_or(0, |(_, packs)| packs);
            if let Some(&at) = seen.get(spot.reference.as_str()) {
                index.placed[at].pieces += 1 + packs;
                continue;
            }
            let source = sources.get(spot.reference.as_str()).copied();
            let base_kind = source.and_then(|s| s.base_kind.as_deref());
            let critter = source.is_some_and(|s| s.critter) || spot.class == SpotClass::Critter;
            let kind = kind_of(base_kind, critter, Some(spot.class));
            if object.is_none() {
                if spot.objects == 0 {
                    empty += 1;
                } else {
                    unmatched += 1;
                }
            }
            if !relevant(kind, source, &spot.reference) {
                continue;
            }
            seen.insert(spot.reference.as_str(), index.placed.len());
            index.placed.push(PlacedRef {
                map: map.stem.to_owned(),
                map_number: map.number,
                elevation: spot.elevation,
                hex: object.map_or(spot.hex, |o| o.hex),
                object_id: object.map(|o| o.id),
                pid: object.map(|o| o.pid),
                kind,
                critter: kind == PlacedKind::Critter,
                pieces: 1 + packs,
                reference: Some(spot.reference.clone()),
                editor_id: source.and_then(|s| s.editor_id.clone()),
                base: source.and_then(|s| s.base.clone()),
                base_kind: source.and_then(|s| s.base_kind.clone()),
                base_editor_id: source.and_then(|s| s.base_editor_id.clone()),
                enable_parent: source.and_then(|s| s.enable_parent.clone()),
                initially_disabled: source.is_some_and(|s| s.initially_disabled),
                hidden: object.is_some_and(|o| o.hidden),
                team: object.and_then(|o| o.team),
                ai_packet: object.and_then(|o| o.ai_packet),
            });
        }
        if moved > 0 {
            index.notes.push(format!(
                "{}: {moved} items or containers the builder moved off a link matched to the \
                 nearest free object within {MOVED_RADIUS} steps",
                map.stem
            ));
        }
        if unmatched > 0 {
            index.notes.push(format!(
                "{}: {unmatched} placements found no object at their hex in the written map",
                map.stem
            ));
        }
        if empty > 0 {
            index.notes.push(format!(
                "{}: {empty} items the catalogue gives no object (a count of 0)",
                map.stem
            ));
        }

        let placed: BTreeSet<&str> = map.spots.iter().map(|s| s.reference.as_str()).collect();
        for source in map.references {
            if placed.contains(source.reference.as_str()) {
                continue;
            }
            let kind = kind_of(source.base_kind.as_deref(), source.critter, None);
            if !relevant(kind, Some(source), &source.reference) {
                continue;
            }
            let (reason, detail) = if let Some(why) = skipped.get(source.reference.as_str()) {
                (UnplacedReason::Skipped, Some((*why).to_owned()))
            } else if source.deleted {
                (UnplacedReason::Deleted, None)
            } else if source.initially_disabled {
                (UnplacedReason::Disabled, None)
            } else {
                (UnplacedReason::NotPlaced, None)
            };
            index.unplaced.push(UnplacedRef {
                map: map.stem.to_owned(),
                reference: source.reference.clone(),
                kind,
                editor_id: source.editor_id.clone(),
                base: source.base.clone(),
                base_kind: source.base_kind.clone(),
                base_editor_id: source.base_editor_id.clone(),
                enable_parent: source.enable_parent.clone(),
                initially_disabled: source.initially_disabled,
                reason,
                detail,
            });
        }
    }
    index.sort();
    index
}

// ---- Reading the written maps ---------------------------------------------

/// One map read back from a written layer.
#[derive(Debug, Clone)]
pub struct WrittenMap {
    /// The file stem.
    pub stem: String,
    /// The map's number in the manifest.
    pub number: u32,
    /// The top-level objects of every elevation, by elevation and file
    /// order (inventories are not listed).
    pub objects: Vec<WrittenObject>,
}

/// The layer's name and its maps, read from `layer_dir` (the folder
/// holding `vault-mod.toml`). Only the manifest, `maps/` and `proto/` are
/// read, never the art.
pub fn read_written_maps(layer_dir: &Path) -> Result<(String, Vec<WrittenMap>), PlacementError> {
    let manifest_path = layer_dir.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| fail(MANIFEST, e))?;
    let manifest = ModManifest::parse(&text).map_err(|e| fail(MANIFEST, e))?;

    let mut entries: Vec<(String, ByteSource)> = Vec::new();
    let mut stack = vec![layer_dir.join("proto")];
    while let Some(folder) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(layer_dir)
                .map_err(|_| fail("proto", "a file outside the layer folder"))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&path).map_err(|e| fail(relative.clone(), e))?;
            let source = ByteSource::new(bytes).map_err(|e| fail(relative.clone(), e))?;
            entries.push((relative, source));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let tree = MemoryDir::new(entries).map_err(|e| fail("proto", e))?;
    let provider = MemoryMount {
        profile: GameProfile::Fallout2,
        loose: Some(tree),
        ..MemoryMount::default()
    }
    .mount()
    .map_err(|e| fail("proto", e))?;
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);

    let mut maps = Vec::new();
    for registration in &manifest.maps {
        let path = format!("maps/{}", registration.file);
        let bytes = std::fs::read(layer_dir.join(&path)).map_err(|e| fail(path.clone(), e))?;
        let parsed = MapFile::parse_for(&bytes, &catalog, GameProfile::Fallout2)
            .map_err(|e| fail(path.clone(), e))?;
        let mut objects = Vec::new();
        for (elevation, list) in parsed.objects.elevations.iter().enumerate() {
            for object in list {
                let Some(hex) = object.hex else { continue };
                let (door, exit_grid, team, ai_packet) = match &object.state {
                    ObjectState::Critter(state) => (false, false, Some(state.team), Some(state.ai)),
                    ObjectState::Other { detail, .. } => (
                        matches!(detail, ObjectDetail::Door(_)),
                        matches!(detail, ObjectDetail::ExitGrid { .. }),
                        None,
                        None,
                    ),
                };
                objects.push(WrittenObject {
                    elevation: elevation as u8,
                    hex,
                    id: object.id,
                    pid: object.pid,
                    critter: object.pid >> 24 == 1,
                    door,
                    exit_grid,
                    holds_items: object.pid >> 24 == 0
                        && (!object.inventory.is_empty() || object.inventory_capacity > 0),
                    hidden: object.flags.contains(BasicFlags::HIDDEN),
                    team,
                    ai_packet,
                });
            }
        }
        let stem = registration
            .file
            .strip_suffix(".map")
            .unwrap_or(&registration.file)
            .to_owned();
        maps.push(WrittenMap {
            stem,
            number: registration.number,
            objects,
        });
    }
    Ok((manifest.name, maps))
}

// ---- The stage's call ------------------------------------------------------

/// What the map stage holds for one map when it writes its output.
pub struct StageMap<'a> {
    /// The map's file stem (`MapRun::stem`; `SliceSpec::map_stem` on the
    /// one-map path).
    pub stem: &'a str,
    /// Every reference of the map's cells the stage considered
    /// (`plan.maps[m].references`, the scene's references on the one-map
    /// path), disabled and deleted ones included.
    pub references: &'a [mir::Reference],
    /// Per elevation, the placements the slicer made, as handed to the
    /// builder (`(run.elevation, run.placements.as_slice())` of each run;
    /// `(0, placements)` on the one-map path).
    pub elevations: Vec<(u8, &'a [map_builder::Placement])>,
    /// The map's build report: its critters and the skipped critters
    /// (`BuiltMap::report`, `Built::report`). The map's number is read
    /// from the written layer.
    pub report: &'a map_builder::BuildReport,
    /// The elevation of each of `report.critters`
    /// (`BuiltMap::critter_elevations`); empty: every critter is on
    /// elevation 0 (the one-map path).
    pub critter_elevations: &'a [u8],
    /// The catalogue handed to the builder (loot on): how many objects
    /// each ground item becomes.
    pub catalogue: Option<&'a map_builder::Catalogue>,
    /// References the stage itself left out, with its words: the slicer's
    /// skips (`Sliced::skipped`, every elevation's) and the people the map
    /// stage did not hand to the builder (`map::PersonSkip`). The first
    /// reason given for a reference is kept; one placed elsewhere is not
    /// listed.
    pub skipped: Vec<(String, String)>,
}

/// A slicer run's skips as [`StageMap::skipped`] takes them.
pub fn slicer_skips(sliced: &map_slicer::Sliced) -> impl Iterator<Item = (String, String)> + '_ {
    sliced
        .skipped
        .iter()
        .map(|s| (s.reference.clone(), format!("slicer: {}", s.reason)))
}

impl<'a> StageMap<'a> {
    /// One map of a layer, from its built map.
    pub fn of_layer(
        stem: &'a str,
        references: &'a [mir::Reference],
        elevations: Vec<(u8, &'a [map_builder::Placement])>,
        built: &'a map_builder::BuiltMap,
        catalogue: Option<&'a map_builder::Catalogue>,
        skipped: Vec<(String, String)>,
    ) -> Self {
        StageMap {
            stem,
            references,
            elevations,
            report: &built.report,
            critter_elevations: &built.critter_elevations,
            catalogue,
            skipped,
        }
    }
}

fn source_of(reference: &mir::Reference) -> SourceRef {
    SourceRef {
        reference: reference.form.to_string(),
        editor_id: reference.editor_id.clone(),
        base: Some(reference.base.to_string()),
        base_kind: reference.base_kind.clone(),
        base_editor_id: reference.base_editor_id.clone(),
        critter: !matches!(reference.kind, mir::RefKind::Object),
        enable_parent: reference.enable_parent.as_ref().map(|p| EnableParent {
            reference: p.reference.to_string(),
            opposite: p.opposite,
        }),
        initially_disabled: reference.flags.initially_disabled,
        deleted: reference.flags.deleted,
    }
}

fn class_of(role: map_builder::Role) -> SpotClass {
    match role {
        map_builder::Role::Door => SpotClass::Door,
        map_builder::Role::Container => SpotClass::Container,
        map_builder::Role::Item => SpotClass::Item,
        _ => SpotClass::Scenery,
    }
}

/// One map's inputs, owned until [`build`] borrows them.
struct Owned {
    stem: String,
    number: u32,
    references: Vec<SourceRef>,
    spots: Vec<Spot>,
    skipped: Vec<(String, String)>,
    written: Vec<WrittenObject>,
}

/// The map stage's one call, after it has written the layer to
/// `layer_dir`: joins its references, placements and critters with the
/// objects of the written maps and writes `<out>/map-placements.json`.
///
/// `referenced` is `mir::Records::referenced_refs()` (references that a
/// script, quest or package points at); an empty set is fine.
pub fn write_for_stage(
    out: &Path,
    layer_dir: &Path,
    game: mir::GameVariant,
    maps: &[StageMap<'_>],
    referenced: &BTreeSet<mir::FormKey>,
) -> Result<PathBuf, PlacementError> {
    let (layer_name, written) = {
        let _t = crate::timings::span("read the written maps");
        read_written_maps(layer_dir)?
    };
    let _t = crate::timings::span("join");
    let referenced: BTreeSet<String> = referenced.iter().map(ToString::to_string).collect();
    let mut owned: Vec<Owned> = Vec::new();
    for map in maps {
        let found = written
            .iter()
            .find(|w| w.stem == map.stem)
            .ok_or_else(|| fail(map.stem, "the map is not in the written layer"))?;
        let mut spots = Vec::new();
        for (elevation, placements) in &map.elevations {
            for placement in *placements {
                let objects = match map
                    .catalogue
                    .and_then(|c| c.placed.get(&placement.reference))
                {
                    Some(map_builder::PlacedLoot::Item { count, .. })
                        if placement.role == map_builder::Role::Item =>
                    {
                        *count
                    }
                    _ => 1,
                };
                spots.push(Spot {
                    reference: placement.reference.clone(),
                    elevation: *elevation,
                    hex: placement.hex,
                    class: class_of(placement.role),
                    objects,
                });
            }
        }
        for (n, critter) in map.report.critters.iter().enumerate() {
            spots.push(Spot {
                reference: critter.reference.clone(),
                elevation: map.critter_elevations.get(n).copied().unwrap_or(0),
                hex: critter.hex,
                class: SpotClass::Critter,
                objects: 1,
            });
        }
        let mut skipped: Vec<(String, String)> = map
            .report
            .skipped_critters
            .iter()
            .map(|s| (s.reference.clone(), s.reason.clone()))
            .collect();
        skipped.extend(map.skipped.iter().cloned());
        owned.push(Owned {
            stem: map.stem.to_owned(),
            number: found.number,
            references: map.references.iter().map(source_of).collect(),
            spots,
            skipped,
            written: found.objects.clone(),
        });
    }
    let inputs: Vec<MapInput<'_>> = owned
        .iter()
        .map(|o| MapInput {
            stem: &o.stem,
            number: o.number,
            references: &o.references,
            spots: &o.spots,
            skipped: &o.skipped,
            written: &o.written,
        })
        .collect();
    let mut index = build(&layer_name, game.id(), &inputs, &referenced);
    // The layer's other maps (the stubs standing in for interiors outside
    // the build) are listed too: they place no source reference.
    let stubs: Vec<&WrittenMap> = written
        .iter()
        .filter(|w| !maps.iter().any(|m| m.stem == w.stem))
        .collect();
    if !stubs.is_empty() {
        for stub in &stubs {
            index.maps.push(MapEntry {
                stem: stub.stem.clone(),
                file: format!("maps/{}.map", stub.stem),
                number: stub.number,
            });
        }
        index.notes.push(format!(
            "{} stub map{} place no source reference",
            stubs.len(),
            if stubs.len() == 1 { "" } else { "s" }
        ));
        index.sort();
    }
    write(out, &index)
}

/// The references a script, quest or package points at
/// (`mir::Records::referenced_refs`), for [`write_for_stage`]; empty, with
/// the reason, when the MIR cannot say (an extract older than records
/// revision 1).
pub fn referenced_of(mir: &mir::Mir) -> (BTreeSet<mir::FormKey>, Option<String>) {
    let _t = crate::timings::span("references scripts, quests and packages name");
    match mir.records().and_then(|r| r.referenced_refs()) {
        Ok(set) => (set, None),
        Err(e) => (BTreeSet::new(), Some(e.to_string())),
    }
}

/// Whether `<out>/map-placements.json` is there and of this schema: the map
/// stage reuses its output only then.
pub fn is_current(out: &Path) -> bool {
    matches!(read(out), Ok(Some(index)) if index.origin == Origin::Stage)
}

// ---- The fallback ------------------------------------------------------------

/// Makes what it can of the index from the written maps alone.
///
/// Every critter, door and container or item with an inventory becomes a
/// [`PlacedRef`] with its map, elevation, hex, object id, PID, hidden flag
/// and (critters) team and AI packet; the form keys, bases and enable
/// parents are absent. An item lying on the ground with no inventory is
/// listed as an item, a container with room for contents as a container;
/// both are guesses from the object alone. Terminals and other scenery
/// cannot be told apart and are not listed. `unplaced` is empty.
///
/// When `out` is given, `features/<stem>.json` is read for the people the
/// map stage listed on an interior map: a critter on the listed hex gets
/// that person's reference.
pub fn reconstruct(
    layer_dir: &Path,
    out: Option<&Path>,
    game: &str,
) -> Result<PlacementIndex, PlacementError> {
    let (layer_name, written) = read_written_maps(layer_dir)?;
    let mut index = PlacementIndex {
        schema_version: SCHEMA_VERSION,
        origin: Origin::Reconstructed,
        layer: layer_name,
        game: game.to_owned(),
        maps: Vec::new(),
        placed: Vec::new(),
        unplaced: Vec::new(),
        notes: vec![
            "reconstructed from the written maps: no references, bases or enable parents, and \
             the kinds are read off the objects"
                .to_owned(),
        ],
    };
    for map in &written {
        index.maps.push(MapEntry {
            stem: map.stem.clone(),
            file: format!("maps/{}.map", map.stem),
            number: map.number,
        });
        let people = out.map(|o| people_of(o, &map.stem)).unwrap_or_default();
        for object in &map.objects {
            let kind = if object.critter {
                PlacedKind::Critter
            } else if object.door {
                PlacedKind::Door
            } else if object.holds_items {
                PlacedKind::Container
            } else if object.pid >> 24 == 0 {
                PlacedKind::Item
            } else {
                continue;
            };
            let named: Vec<&String> = people
                .iter()
                .filter(|(hex, _)| *hex == object.hex)
                .map(|(_, reference)| reference)
                .collect();
            let reference = match (kind, named.as_slice()) {
                (PlacedKind::Critter, [one]) => Some((*one).clone()),
                _ => None,
            };
            index.placed.push(PlacedRef {
                map: map.stem.clone(),
                map_number: map.number,
                elevation: object.elevation,
                hex: object.hex,
                object_id: Some(object.id),
                pid: Some(object.pid),
                kind,
                critter: object.critter,
                pieces: 1,
                reference,
                editor_id: None,
                base: None,
                base_kind: None,
                base_editor_id: None,
                enable_parent: None,
                initially_disabled: false,
                hidden: object.hidden,
                team: object.team,
                ai_packet: object.ai_packet,
            });
        }
    }
    index.sort();
    Ok(index)
}

/// `(hex, reference)` of the people `features/<stem>.json` lists; empty
/// when the file is absent or has none.
fn people_of(out: &Path, stem: &str) -> Vec<(i32, String)> {
    let path = out.join("features").join(format!("{stem}.json"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("people")
        .and_then(|p| p.as_array())
        .map(|people| {
            people
                .iter()
                .filter_map(|p| {
                    Some((
                        i32::try_from(p.get("hex")?.as_i64()?).ok()?,
                        p.get("reference")?.as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(reference: &str, base_kind: &str) -> SourceRef {
        SourceRef {
            reference: reference.to_owned(),
            editor_id: Some(format!("ed{reference}")),
            base: Some(format!("base:{reference}")),
            base_kind: Some(base_kind.to_owned()),
            base_editor_id: None,
            critter: matches!(base_kind, "NPC_" | "CREA"),
            enable_parent: None,
            initially_disabled: false,
            deleted: false,
        }
    }

    fn spot(reference: &str, elevation: u8, hex: i32, class: SpotClass) -> Spot {
        Spot {
            reference: reference.to_owned(),
            elevation,
            hex,
            class,
            objects: 1,
        }
    }

    fn object(elevation: u8, hex: i32, id: u32) -> WrittenObject {
        WrittenObject {
            elevation,
            hex,
            id,
            pid: 0x0200_0000 + id,
            critter: false,
            door: false,
            exit_grid: false,
            holds_items: false,
            hidden: false,
            team: None,
            ai_packet: None,
        }
    }

    fn critter(elevation: u8, hex: i32, id: u32, team: i32) -> WrittenObject {
        WrittenObject {
            pid: 0x0100_0000 + id,
            critter: true,
            team: Some(team),
            ai_packet: Some(3),
            ..object(elevation, hex, id)
        }
    }

    fn door(elevation: u8, hex: i32, id: u32) -> WrittenObject {
        WrittenObject {
            door: true,
            ..object(elevation, hex, id)
        }
    }

    struct Data {
        references: Vec<SourceRef>,
        spots: Vec<Spot>,
        skipped: Vec<(String, String)>,
        written: Vec<WrittenObject>,
    }

    fn data() -> Data {
        let mut child = source("p:000007", "STAT");
        child.enable_parent = Some(EnableParent {
            reference: "p:000006".to_owned(),
            opposite: true,
        });
        let mut disabled_npc = source("p:000008", "NPC_");
        disabled_npc.initially_disabled = true;
        let mut deleted_door = source("p:000009", "DOOR");
        deleted_door.deleted = true;
        let mut parent = source("p:000006", "ACTI");
        parent.initially_disabled = true;
        Data {
            references: vec![
                source("p:000001", "DOOR"),
                source("p:000002", "NPC_"),
                source("p:000003", "CONT"),
                source("p:000004", "WEAP"),
                source("p:000005", "STAT"),
                parent,
                child,
                disabled_npc,
                deleted_door,
                source("p:00000a", "CREA"),
                source("p:00000b", "TERM"),
                source("p:00000c", "MISC"),
            ],
            spots: vec![
                spot("p:000005", 0, 100, SpotClass::Scenery),
                spot("p:000001", 0, 100, SpotClass::Door),
                spot("p:000003", 0, 205, SpotClass::Container),
                spot("p:000004", 0, 205, SpotClass::Item),
                spot("p:000007", 1, 300, SpotClass::Scenery),
                spot("p:000007", 1, 301, SpotClass::Scenery),
                spot("p:000002", 0, 150, SpotClass::Critter),
                spot("p:00000b", 0, 400, SpotClass::Scenery),
            ],
            skipped: vec![("p:00000a".to_owned(), "no sprite set".to_owned())],
            written: vec![
                object(0, 100, 1),
                door(0, 100, 2),
                WrittenObject {
                    pid: 0x0000_0103,
                    ..object(0, 205, 3)
                },
                WrittenObject {
                    pid: 0x0000_0104,
                    ..object(0, 205, 4)
                },
                object(1, 300, 5),
                object(1, 301, 6),
                critter(0, 150, 7, 4),
                WrittenObject {
                    exit_grid: true,
                    ..object(0, 400, 8)
                },
                object(0, 400, 9),
            ],
        }
    }

    fn built(d: &Data) -> PlacementIndex {
        let maps = [MapInput {
            stem: "town",
            number: 151,
            references: &d.references,
            spots: &d.spots,
            skipped: &d.skipped,
            written: &d.written,
        }];
        build("Test layer", "fo3", &maps, &BTreeSet::new())
    }

    #[test]
    fn the_join_gives_each_placement_its_own_object() {
        let index = built(&data());
        let find = |r: &str| {
            index
                .placed
                .iter()
                .find(|p| p.reference.as_deref() == Some(r))
                .unwrap_or_else(|| panic!("{r} missing"))
        };
        // The plain static at hex 100 is not listed, but it took object 1:
        // the door gets object 2.
        assert_eq!(find("p:000001").object_id, Some(2));
        assert_eq!(find("p:000001").kind, PlacedKind::Door);
        assert_eq!(find("p:000003").object_id, Some(3));
        assert_eq!(find("p:000004").object_id, Some(4));
        assert_eq!(find("p:000004").kind, PlacedKind::Item);
        let npc = find("p:000002");
        assert_eq!(
            (npc.object_id, npc.team, npc.ai_packet),
            (Some(7), Some(4), Some(3))
        );
        assert!(npc.critter);
        // The terminal skips the exit grid and takes the object after it.
        assert_eq!(find("p:00000b").object_id, Some(9));
        assert_eq!(find("p:00000b").kind, PlacedKind::Terminal);
        assert!(index.notes.is_empty(), "{:?}", index.notes);
    }

    #[test]
    fn scenery_is_listed_only_when_enable_state_needs_it() {
        let index = built(&data());
        let refs: Vec<&str> = index
            .placed
            .iter()
            .filter_map(|p| p.reference.as_deref())
            .collect();
        assert!(!refs.contains(&"p:000005"), "{refs:?}");
        let child = index
            .placed
            .iter()
            .find(|p| p.reference.as_deref() == Some("p:000007"))
            .expect("the enable child");
        assert_eq!(child.kind, PlacedKind::Scenery);
        assert_eq!(child.pieces, 2);
        assert_eq!(
            (child.elevation, child.hex, child.object_id),
            (1, 300, Some(5))
        );
        assert_eq!(child.enable_parent.as_ref().map(|p| p.opposite), Some(true));
        // A caller may name more.
        let d = data();
        let maps = [MapInput {
            stem: "town",
            number: 151,
            references: &d.references,
            spots: &d.spots,
            skipped: &d.skipped,
            written: &d.written,
        }];
        let named: BTreeSet<String> = ["p:000005".to_owned()].into();
        let index = build("Test layer", "fo3", &maps, &named);
        assert!(
            index
                .placed
                .iter()
                .any(|p| p.reference.as_deref() == Some("p:000005"))
        );
    }

    #[test]
    fn what_is_left_off_is_listed_with_its_reason() {
        let index = built(&data());
        let reason = |r: &str| {
            index
                .unplaced
                .iter()
                .find(|u| u.reference == r)
                .map(|u| (u.reason, u.detail.clone()))
        };
        assert_eq!(reason("p:000008"), Some((UnplacedReason::Disabled, None)));
        assert_eq!(reason("p:000009"), Some((UnplacedReason::Deleted, None)));
        assert_eq!(
            reason("p:00000a"),
            Some((UnplacedReason::Skipped, Some("no sprite set".to_owned())))
        );
        assert_eq!(reason("p:00000c"), Some((UnplacedReason::NotPlaced, None)));
        // The disabled enable parent is a scenery reference that matters.
        assert_eq!(reason("p:000006"), Some((UnplacedReason::Disabled, None)));
        // Placed or irrelevant references are not there.
        assert_eq!(reason("p:000001"), None);
        assert_eq!(reason("p:000005"), None);
    }

    #[test]
    fn the_lists_are_sorted_by_map_elevation_and_object_id() {
        let index = built(&data());
        let keys: Vec<(String, u8, Option<u32>)> = index
            .placed
            .iter()
            .map(|p| (p.map.clone(), p.elevation, p.object_id))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(keys.first().map(|k| k.2), Some(Some(2)));
        let unplaced: Vec<&str> = index
            .unplaced
            .iter()
            .map(|u| u.reference.as_str())
            .collect();
        let mut sorted = unplaced.clone();
        sorted.sort_unstable();
        assert_eq!(unplaced, sorted);
    }

    #[test]
    fn two_builds_are_byte_identical_whatever_the_input_order() {
        let a = data();
        let mut b = data();
        b.references.reverse();
        b.skipped.reverse();
        assert_eq!(built(&a).to_text(), built(&a).to_text());
        assert_eq!(built(&a).to_text(), built(&b).to_text());
        let text = built(&a).to_text();
        assert!(text.ends_with("}\n"));
        assert!(!text.contains('\\'), "no path separators: {text}");
    }

    #[test]
    fn the_file_round_trips_and_checks_its_version() {
        let index = built(&data());
        let again = PlacementIndex::from_text(&index.to_text()).expect("parses");
        assert_eq!(again, index);
        assert_eq!(again.to_text(), index.to_text());

        let dir = tempfile::tempdir().expect("temp dir");
        assert!(read(dir.path()).expect("absent is fine").is_none());
        let path = write(dir.path(), &index).expect("writes");
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some(FILE_NAME));
        assert_eq!(read(dir.path()).expect("reads"), Some(index.clone()));

        let stale = index
            .to_text()
            .replace("\"schema_version\": 1", "\"schema_version\": 9");
        let error = PlacementIndex::from_text(&stale).expect_err("stale");
        assert!(error.to_string().contains("schema version 9"), "{error}");
    }

    #[test]
    fn unknown_fields_are_absent_not_null() {
        let index = built(&data());
        let text = index.to_text();
        assert!(!text.contains("null"), "{text}");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let first = &value["placed"][0];
        assert_eq!(first["kind"], "door");
        assert!(first.get("team").is_none());
        assert_eq!(value["origin"], "stage");
    }

    #[test]
    fn packs_and_items_moved_off_a_link_find_their_objects() {
        let mut d = data();
        // Rounds at 800 are three packs (consecutive ids 20 to 22, one
        // PID); rounds placed at 500 the builder moved to 702, where
        // nothing else was placed; an object at 9000 is too far.
        let item = |hex: i32, id: u32, pid: u32| WrittenObject {
            pid,
            ..object(0, hex, id)
        };
        d.references.push(source("p:00000e", "AMMO"));
        d.spots.push(Spot {
            objects: 3,
            ..spot("p:00000e", 0, 800, SpotClass::Item)
        });
        d.written.extend([
            item(800, 20, 0x77),
            item(800, 21, 0x77),
            item(800, 22, 0x77),
        ]);
        d.references.push(source("p:00000d", "AMMO"));
        d.spots.push(spot("p:00000d", 0, 500, SpotClass::Item));
        d.written
            .extend([item(702, 23, 0x78), item(9000, 24, 0x79)]);
        let index = built(&d);
        let find = |r: &str| {
            index
                .placed
                .iter()
                .find(|p| p.reference.as_deref() == Some(r))
                .unwrap_or_else(|| panic!("{r} missing"))
        };
        assert_eq!(
            (find("p:00000e").object_id, find("p:00000e").pieces),
            (Some(20), 3)
        );
        assert_eq!(find("p:000004").pieces, 1);
        let moved = find("p:00000d");
        assert_eq!((moved.object_id, moved.hex), (Some(23), 702));
        assert!(
            index.notes.iter().any(|n| n.contains("moved off a link")),
            "{:?}",
            index.notes
        );
        // Without the count, two placements of one item at a hex take one
        // object each.
        let mut d = data();
        d.references.push(source("p:00000f", "AMMO"));
        d.references.push(source("p:000010", "AMMO"));
        d.spots.push(spot("p:00000f", 0, 800, SpotClass::Item));
        d.spots.push(spot("p:000010", 0, 800, SpotClass::Item));
        d.written.extend([item(800, 20, 0x77), item(800, 21, 0x77)]);
        let index = built(&d);
        let id = |r: &str| {
            index
                .placed
                .iter()
                .find(|p| p.reference.as_deref() == Some(r))
                .and_then(|p| p.object_id)
        };
        assert_eq!((id("p:00000f"), id("p:000010")), (Some(20), Some(21)));
    }

    #[test]
    fn a_placement_without_an_object_is_counted_not_dropped() {
        let mut d = data();
        d.written.retain(|o| o.id != 3);
        let index = built(&d);
        let container = index
            .placed
            .iter()
            .find(|p| p.reference.as_deref() == Some("p:000003"))
            .expect("listed");
        // Object 4 is the only one left at the hex and it goes to the
        // container, the first placement there.
        assert_eq!(container.object_id, Some(4));
        let item = index
            .placed
            .iter()
            .find(|p| p.reference.as_deref() == Some("p:000004"))
            .expect("listed");
        assert_eq!(item.object_id, None);
        assert!(index.notes.iter().any(|n| n.contains("1 placements")));
    }
}
