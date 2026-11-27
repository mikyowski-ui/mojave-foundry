//! The placement index: `map-placements.json`, written beside
//! `map-report.md` (not inside the layer), the one file the simulation
//! stage needs to find, on the written maps, the references the source
//! plugins talk about (`docs/simulation-layer-spec.md`, section 10.4).
//!
//! # Schema (version 2)
//!
//! ```text
//! { schema_version, origin ("stage" | "reconstructed"), layer, game,
//!   maps:     [ { stem, file, number } ],
//!   placed:   [ PlacedRef ],
//!   unplaced: [ UnplacedRef ],
//!   triggers: [ TriggerRef ],
//!   notes:    [ text ] }
//! ```
//!
//! A [`PlacedRef`] is one reference that became an object of a written
//! map: its `map` (stem), `map_number`, `elevation`, `hex`, `object_id`
//! and `pid` as the map file has them (the builder returns the exact
//! reference-to-object map, `map_builder::BuiltMap::objects`; nothing is
//! guessed from positions); `kind` and `critter`; and, when the source is
//! known, the `reference` (form key), `editor_id`, `base`, `base_kind`
//! (record signature), `base_editor_id`, `enable_parent` (`reference`,
//! `opposite`), `group` (the enable group: its root, the opposite flag
//! multiplied along the chain, and whether the group starts enabled),
//! `initially_disabled` (the source's own flag), `hidden` (the written
//! object carries the hidden flag), `lock` (what the source locks, and
//! whether the written object is locked), `linked_ref` (`XLKR`), and for
//! critters `team`, `ai_packet`, `hostile` and, for a presence copy,
//! `presence_of`. A field the writer cannot know is left out of the JSON;
//! optional fields are omitted, never `null`.
//!
//! An [`UnplacedRef`] is a reference of a map's cells that is not on the
//! map, with the `reason` (`disabled`, `deleted`, `skipped`, `not_placed`)
//! and an optional `detail`: enable-parent state needs to know them too.
//!
//! A [`TriggerRef`] is a trigger volume (a primitive on an activator): no
//! object stands for it, the `sim` stage attaches a spatial script at its
//! `hex` with its `radius` (hexes) and `shape`, from the primitive the MIR
//! carries (`mir::Reference::primitive`, schema 3). The radius covers the
//! footprint whatever the shape; a box is not tested more exactly.
//!
//! Both lists hold the references the simulation can use: critters, doors,
//! containers, items and terminals, and any other reference that is an
//! enable parent or child, is linked (`XLKR`, either end), or that the
//! caller names (referenced by a script, a quest or a package). Plain
//! scenery is not listed. All lists are sorted, and nothing in the file
//! depends on the time, the host or a path, so two builds from the same
//! inputs are byte-identical.
//!
//! # Two ways to make it
//!
//! - [`write_for_stage`] is the map stage's call: it joins the stage's own
//!   references and the builder's exact object map with the objects read
//!   back from the written maps (`build`, a pure function of plain
//!   inputs). An id the map file does not bear out (a different hex or
//!   PID) is counted in the notes.
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
use vault_formats::pro::{BasicFlags, LockFlags};
use vault_formats::pro_catalog::LocalPrototypeCatalog;
use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};

/// The file's name, under the run's output directory.
pub const FILE_NAME: &str = "map-placements.json";

/// The schema version [`PlacementIndex::schema_version`] carries.
pub const SCHEMA_VERSION: u32 = 2;

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
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
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
    #[default]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Written by the map stage, from its own records.
    #[default]
    Stage,
    /// Made from the written maps alone ([`reconstruct`]).
    Reconstructed,
}

/// One map of the layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapEntry {
    /// The file stem (`sdmart` for `maps/sdmart.map`).
    pub stem: String,
    /// `maps/<stem>.map`.
    pub file: String,
    /// The map's number in the layer's manifest.
    pub number: u32,
    /// The readability turn the map was built with, degrees clockwise
    /// seen from above (`world_yaw_deg`): its hexes are the turned world's.
    /// Left out for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_yaw_deg: Option<f64>,
}

impl Eq for MapEntry {}

impl Ord for MapEntry {
    /// By stem, file and number (then the turn, by its bits).
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.stem, &self.file, self.number)
            .cmp(&(&other.stem, &other.file, other.number))
            .then_with(|| {
                let bits = |v: Option<f64>| v.map(f64::to_bits);
                bits(self.world_yaw_deg).cmp(&bits(other.world_yaw_deg))
            })
    }
}

impl PartialOrd for MapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// An enable parent, as the source states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnableParent {
    /// The parent reference's form key.
    pub reference: String,
    /// The child is enabled when the parent is disabled.
    pub opposite: bool,
}

/// The enable group a reference belongs to (spec 8.1): a parent with at
/// least one child, or a child; a chain is flattened to its root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInfo {
    /// The root's form key: the group's state is the root's.
    pub root: String,
    /// The opposite flags multiplied along the chain to the root: the
    /// reference is enabled when the root is disabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub opposite: bool,
    /// Whether the reference starts enabled: the root's own flag, flipped
    /// by `opposite` for a child (a child follows its parent, not its own
    /// flag).
    pub initially_enabled: bool,
}

/// A lock on a door or container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    /// The written object carries the lock bit (the lock is written with
    /// the catalogue, `loot = true`).
    pub locked: bool,
    /// The source's lock level, as stored (0 very easy to 255 key only).
    pub level: u8,
    /// The key's base record (a form key), when the lock names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The Fallout 2 prototype id of the converted key item, when the
    /// items catalogue made one for `key`; left out when the lock names no
    /// key or the key is not in the layer (the note `keys_missing` counts
    /// those).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_pid: Option<u32>,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedRef {
    /// The map's file stem.
    pub map: String,
    /// The map's number.
    pub map_number: u32,
    /// The elevation, 0 to 2.
    pub elevation: u8,
    /// The hex, 0 to 39,999.
    pub hex: i32,
    /// The object's id in the written map; absent when no object was found
    /// (the note says how many).
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
    /// Every object id of a reference that became several objects, the
    /// first included, in write order; left out for one object.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub piece_ids: Vec<u32>,
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
    /// The enable group, for a parent or a child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupInfo>,
    /// The source starts the reference disabled (its own flag).
    #[serde(default, skip_serializing_if = "is_false")]
    pub initially_disabled: bool,
    /// The written object carries the hidden flag.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// The source's lock, on a door or container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<LockInfo>,
    /// The linked reference (`XLKR`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_ref: Option<String>,
    /// A critter's team in the written map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    /// A critter's AI packet number in the written map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_packet: Option<i32>,
    /// The catalogue marks the critter hostile (its packet is the hostile
    /// packet).
    #[serde(default, skip_serializing_if = "is_false")]
    pub hostile: bool,
    /// A presence copy (spec 7.6): the reference of the actor it copies.
    /// Its `reference` is that one, `@`, and the map's stem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_of: Option<String>,
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
    /// The enable group, for a parent or a child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupInfo>,
    /// The source starts the reference disabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub initially_disabled: bool,
    /// The source's lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<LockInfo>,
    /// The linked reference (`XLKR`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_ref: Option<String>,
    /// Why.
    pub reason: UnplacedReason,
    /// The stage's own words, when it gave a reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A trigger volume: a primitive on an activator. No object stands for it;
/// the `sim` stage attaches a spatial script there (spec 5.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerRef {
    /// The map holding the volume's cell (file stem) and its number.
    pub map: String,
    pub map_number: u32,
    /// The elevation, and the hex under the volume's centre.
    pub elevation: u8,
    pub hex: i32,
    /// The reference's form key.
    pub reference: String,
    /// `box` or `sphere`; absent when the reference has no primitive of
    /// either shape (a MIR written before schema 3 has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// The smallest hex radius around `hex` that covers the volume's
    /// footprint, with a corner off the map pulled in to the map's edge;
    /// absent without a box or sphere primitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_editor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_parent: Option<EnableParent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupInfo>,
    /// The linked reference (`XLKR`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_ref: Option<String>,
}

/// Where a [`MarkerRef`] comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerSource {
    /// A reference of the map's own cells that is not on the map (the
    /// slicer skips markers, declutter prunes, a static is left out): the
    /// index lists it in `unplaced` too.
    Cell,
    /// A reference of a cell outside the maps' cells, named as a package's
    /// location or target, whose position lies on the map's window.
    Outside,
}

/// A position with no object (task T9): where a reference would stand on
/// a map, projected with the map stage's own world-to-hex projection
/// (`map_builder::walk::Frame::hex_of_source`), on the elevation whose
/// run holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerRef {
    /// The map's stem and number.
    pub map: String,
    pub map_number: u32,
    /// The elevation and the hex.
    pub elevation: u8,
    pub hex: i32,
    /// The reference's form key.
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_editor_id: Option<String>,
    /// In the map's cells or outside them.
    pub source: MarkerSource,
}

/// A projected position the map stage hands [`write_for_stage`]: a
/// reference, its elevation and hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerSpot {
    /// The reference's form key.
    pub reference: String,
    pub elevation: u8,
    pub hex: i32,
}

/// A reference outside the maps' cells, as [`outside_markers`] finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideSpot {
    /// Where it projects.
    pub spot: MarkerSpot,
    /// Its base form key and signature, from the MIR.
    pub base: Option<String>,
    pub base_kind: Option<String>,
}

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlacementIndex {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Who made it.
    pub origin: Origin,
    /// The layer's name, from its manifest.
    pub layer: String,
    /// The source game, as `mir::GameVariant::id` names it (`fallout3`).
    pub game: String,
    /// The readability turn of the start map (the first), degrees
    /// clockwise seen from above, when any map was built turned
    /// (`MapEntry::world_yaw_deg` gives each map's): its hexes are those of
    /// the turned world. Left out when no map is turned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_yaw_deg: Option<f64>,
    /// The layer's maps, by number.
    pub maps: Vec<MapEntry>,
    /// The references on the maps, by map, elevation and object id.
    pub placed: Vec<PlacedRef>,
    /// The references of the maps' cells that are not on them.
    pub unplaced: Vec<UnplacedRef>,
    /// The trigger volumes of the maps' cells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerRef>,
    /// Positions with no object (task T9): markers and other scenery the
    /// stage does not place, and references outside the maps' cells that
    /// a package names, each with the hex and elevation it would stand
    /// on. An optional addition to schema 2 (absent in a file written
    /// before it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<MarkerRef>,
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
        self.triggers.sort_by(|a, b| {
            (&a.map, a.elevation, a.hex, &a.reference).cmp(&(
                &b.map,
                b.elevation,
                b.hex,
                &b.reference,
            ))
        });
        self.markers.sort_by(|a, b| {
            (&a.map, a.elevation, a.hex, &a.reference).cmp(&(
                &b.map,
                b.elevation,
                b.hex,
                &b.reference,
            ))
        });
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

// ---- Enable groups -----------------------------------------------------------

/// What [`group_members`] needs to know about one reference.
#[derive(Debug, Clone, Copy)]
pub struct GroupSource<'a> {
    /// The reference's form key.
    pub form: &'a str,
    /// Its enable parent's form key and the opposite flag.
    pub parent: Option<(&'a str, bool)>,
    /// The reference's own initially-disabled flag.
    pub own_disabled: bool,
}

/// A member of an enable group (spec 8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The root of its chain (a parent that is not itself a child, or one
    /// outside the references given).
    pub root: String,
    /// The opposite flags multiplied along the chain.
    pub opposite: bool,
    /// The reference has an enable parent.
    pub has_parent: bool,
    /// Whether it starts disabled: for a child, its parent's state flipped
    /// by the opposite flag (a child follows its parent; its own flag is
    /// not read), for a parent its own flag. A parent outside the
    /// references given counts as enabled.
    pub starts_disabled: bool,
}

/// The enable groups among `refs`: every reference with an enable parent,
/// and every reference that is some reference's parent. A chain is
/// flattened to its root; a cycle is cut (its members count as enabled).
pub fn group_members(refs: &[GroupSource<'_>]) -> BTreeMap<String, Member> {
    const DEPTH: usize = 64;
    let by: BTreeMap<&str, &GroupSource<'_>> = refs.iter().map(|r| (r.form, r)).collect();
    let parents: BTreeSet<&str> = refs
        .iter()
        .filter_map(|r| r.parent.map(|(parent, _)| parent))
        .collect();
    let enabled = |form: &str| -> bool {
        let mut flips = false;
        let mut current = form;
        for _ in 0..DEPTH {
            let Some(r) = by.get(current) else {
                return !flips;
            };
            match r.parent {
                Some((parent, opposite)) => {
                    flips ^= opposite;
                    current = parent;
                }
                None => return !r.own_disabled ^ flips,
            }
        }
        true
    };
    let mut out = BTreeMap::new();
    for r in refs {
        let has_parent = r.parent.is_some();
        if !has_parent && !parents.contains(r.form) {
            continue;
        }
        let mut opposite = false;
        let mut root = r.form;
        for _ in 0..DEPTH {
            let Some((parent, o)) = by.get(root).and_then(|r| r.parent) else {
                break;
            };
            opposite ^= o;
            root = parent;
        }
        out.insert(
            r.form.to_owned(),
            Member {
                root: root.to_owned(),
                opposite,
                has_parent,
                starts_disabled: !enabled(r.form),
            },
        );
    }
    out
}

fn group_info(member: &Member) -> GroupInfo {
    GroupInfo {
        root: member.root.clone(),
        opposite: member.opposite,
        initially_enabled: !member.starts_disabled,
    }
}

// ---- The pure builder ----------------------------------------------------

/// What the join needs to know about a reference of a map's cells.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// The source's lock: level and key base.
    pub lock: Option<(u8, Option<String>)>,
    /// The linked reference (`XLKR`).
    pub linked_ref: Option<String>,
}

/// One object a reference became, as the builder reports it
/// (`map_builder::ObjectRef`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSpot {
    /// The reference (a presence copy's own key).
    pub reference: String,
    /// Its elevation.
    pub elevation: u8,
    /// The object id in the map file.
    pub id: u32,
    /// The hex and PID the builder wrote it with.
    pub hex: i32,
    pub pid: u32,
}

/// A trigger volume of a map, placed by the stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerSpot {
    /// The reference's form key.
    pub reference: String,
    pub elevation: u8,
    /// The hex under its centre.
    pub hex: i32,
    /// `box` or `sphere`, when known.
    pub shape: Option<String>,
    /// The hex radius that covers its footprint, when known.
    pub radius: Option<u32>,
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
    /// The lock bit is set (a door's state word, a container's item word).
    pub locked: bool,
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
    /// What the builder made of the stage's placements and critters, in
    /// write order.
    pub objects: &'a [ObjectSpot],
    /// References the stage skipped and its reason for each.
    pub skipped: &'a [(String, String)],
    /// The objects of the written map.
    pub written: &'a [WrittenObject],
    /// Presence copies on this map: the copy's reference and the original's.
    pub copies: &'a [(String, String)],
    /// The map's trigger volumes.
    pub triggers: &'a [TriggerSpot],
    /// Critter references the catalogue marks hostile.
    pub hostile: &'a BTreeSet<String>,
    /// The PID of each key item the layer holds, by the key's base form
    /// key ([`key_pids_of`]).
    pub key_pids: &'a BTreeMap<String, u32>,
}

fn kind_of(base_kind: Option<&str>, critter: bool, written: Option<&WrittenObject>) -> PlacedKind {
    if critter {
        return PlacedKind::Critter;
    }
    if let Some(kind) = base_kind.and_then(PlacedKind::from_signature) {
        return kind;
    }
    match written {
        Some(w) if w.critter => PlacedKind::Critter,
        Some(w) if w.door => PlacedKind::Door,
        Some(w) if w.pid >> 24 == 0 && w.holds_items => PlacedKind::Container,
        Some(w) if w.pid >> 24 == 0 => PlacedKind::Item,
        _ => PlacedKind::Scenery,
    }
}

/// Builds the index from plain inputs.
///
/// `referenced` names references that are listed whatever their kind
/// (those a script, quest or package points at: `mir::Records::referenced_refs`).
///
/// Each reference's objects are the builder's exact list: the first gives
/// the entry's `object_id`, `pid` and `hex` (read from `written`, which
/// must bear out the builder's hex and PID, else the notes count the
/// disagreement), the count its `pieces`. Presence copies are placed
/// entries of their own, described as their original with the
/// original's enable and lock data left off.
pub fn build(
    layer: &str,
    game: &str,
    maps: &[MapInput<'_>],
    referenced: &BTreeSet<String>,
) -> PlacementIndex {
    let all: BTreeMap<&str, &SourceRef> = maps
        .iter()
        .flat_map(|m| m.references.iter())
        .map(|r| (r.reference.as_str(), r))
        .collect();
    let group_sources: Vec<GroupSource<'_>> = all
        .values()
        .map(|r| GroupSource {
            form: &r.reference,
            parent: r
                .enable_parent
                .as_ref()
                .map(|p| (p.reference.as_str(), p.opposite)),
            own_disabled: r.initially_disabled,
        })
        .collect();
    let groups = group_members(&group_sources);
    let linked: BTreeSet<&str> = all
        .values()
        .filter_map(|r| r.linked_ref.as_deref())
        .collect();
    let relevant = |kind: PlacedKind, source: Option<&SourceRef>, reference: &str| {
        kind != PlacedKind::Scenery
            || groups.contains_key(reference)
            || source.is_some_and(|s| s.linked_ref.is_some())
            || linked.contains(reference)
            || referenced.contains(reference)
    };
    let lock_of = |source: Option<&SourceRef>,
                   written: Option<&WrittenObject>,
                   key_pids: &BTreeMap<String, u32>| {
        source
            .and_then(|s| s.lock.as_ref())
            .map(|(level, key)| LockInfo {
                locked: written.is_some_and(|w| w.locked),
                level: *level,
                key: key.clone(),
                key_pid: key.as_ref().and_then(|k| key_pids.get(k)).copied(),
            })
    };
    let mut index = PlacementIndex {
        schema_version: SCHEMA_VERSION,
        origin: Origin::Stage,
        layer: layer.to_owned(),
        game: game.to_owned(),
        world_yaw_deg: None,
        maps: Vec::new(),
        placed: Vec::new(),
        unplaced: Vec::new(),
        triggers: Vec::new(),
        markers: Vec::new(),
        notes: Vec::new(),
    };
    for map in maps {
        index.maps.push(MapEntry {
            stem: map.stem.to_owned(),
            file: format!("maps/{}.map", map.stem),
            number: map.number,
            world_yaw_deg: None,
        });
        let sources: BTreeMap<&str, &SourceRef> = map
            .references
            .iter()
            .map(|r| (r.reference.as_str(), r))
            .collect();
        let copies: BTreeMap<&str, &str> = map
            .copies
            .iter()
            .map(|(copy, original)| (copy.as_str(), original.as_str()))
            .collect();
        // The first reason given for a reference.
        let mut skipped: BTreeMap<&str, &str> = BTreeMap::new();
        for (r, why) in map.skipped {
            skipped.entry(r.as_str()).or_insert(why.as_str());
        }
        let written: BTreeMap<(u8, u32), &WrittenObject> = map
            .written
            .iter()
            .map(|o| ((o.elevation, o.id), o))
            .collect();

        // The builder's objects, by reference, in write order.
        let mut order: Vec<&str> = Vec::new();
        let mut spots: BTreeMap<&str, Vec<&ObjectSpot>> = BTreeMap::new();
        for spot in map.objects {
            let list = spots.entry(spot.reference.as_str()).or_default();
            if list.is_empty() {
                order.push(spot.reference.as_str());
            }
            list.push(spot);
        }
        let mut unmatched = 0usize;
        let mut disagree = 0usize;
        let mut placed_refs: BTreeSet<&str> = BTreeSet::new();
        for reference in order {
            let list = &spots[reference];
            let first = list[0];
            let object = written.get(&(first.elevation, first.id)).copied();
            match object {
                None => unmatched += 1,
                Some(w) if w.hex != first.hex || w.pid != first.pid => disagree += 1,
                Some(_) => {}
            }
            let original = copies.get(reference).copied();
            let source = match original {
                Some(original) => all.get(original).copied(),
                None => sources
                    .get(reference)
                    .copied()
                    .or_else(|| all.get(reference).copied()),
            };
            let base_kind = source.and_then(|s| s.base_kind.as_deref());
            let critter = source.is_some_and(|s| s.critter) || object.is_some_and(|w| w.critter);
            let kind = kind_of(base_kind, critter, object);
            placed_refs.insert(reference);
            if original.is_none() && !relevant(kind, source, reference) {
                continue;
            }
            // A copy is its original's twin, without the original's
            // enable, lock or link data.
            let own = original.is_none();
            index.placed.push(PlacedRef {
                map: map.stem.to_owned(),
                map_number: map.number,
                elevation: first.elevation,
                hex: object.map_or(first.hex, |o| o.hex),
                object_id: Some(first.id),
                pid: Some(object.map_or(first.pid, |o| o.pid)),
                kind,
                critter: kind == PlacedKind::Critter,
                pieces: list.len() as u32,
                piece_ids: if list.len() > 1 {
                    list.iter().map(|s| s.id).collect()
                } else {
                    Vec::new()
                },
                reference: Some(reference.to_owned()),
                editor_id: source.and_then(|s| s.editor_id.clone()),
                base: source.and_then(|s| s.base.clone()),
                base_kind: source.and_then(|s| s.base_kind.clone()),
                base_editor_id: source.and_then(|s| s.base_editor_id.clone()),
                enable_parent: source.filter(|_| own).and_then(|s| s.enable_parent.clone()),
                group: source
                    .filter(|_| own)
                    .and_then(|s| groups.get(&s.reference))
                    .map(group_info),
                initially_disabled: own && source.is_some_and(|s| s.initially_disabled),
                hidden: object.is_some_and(|o| o.hidden),
                lock: lock_of(source.filter(|_| own), object, map.key_pids),
                linked_ref: source.filter(|_| own).and_then(|s| s.linked_ref.clone()),
                team: object.and_then(|o| o.team),
                ai_packet: object.and_then(|o| o.ai_packet),
                hostile: map.hostile.contains(reference),
                presence_of: original.map(str::to_owned),
            });
        }
        let keys_missing = index
            .placed
            .iter()
            .filter(|p| p.map == map.stem)
            .filter_map(|p| p.lock.as_ref())
            .filter(|l| l.key.is_some() && l.key_pid.is_none())
            .count();
        if keys_missing > 0 {
            index.notes.push(format!(
                "{}: keys_missing {keys_missing}: locks name a key item the layer does not hold",
                map.stem
            ));
        }
        if unmatched > 0 {
            index.notes.push(format!(
                "{}: {unmatched} references name an object id the written map does not hold",
                map.stem
            ));
        }
        if disagree > 0 {
            index.notes.push(format!(
                "{}: {disagree} references name an object whose hex or PID the written map \
                 does not bear out",
                map.stem
            ));
        }

        for source in map.references {
            if placed_refs.contains(source.reference.as_str()) {
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
                group: groups.get(&source.reference).map(group_info),
                initially_disabled: source.initially_disabled,
                lock: lock_of(Some(source), None, map.key_pids),
                linked_ref: source.linked_ref.clone(),
                reason,
                detail,
            });
        }

        for trigger in map.triggers {
            let source = sources.get(trigger.reference.as_str()).copied();
            index.triggers.push(TriggerRef {
                map: map.stem.to_owned(),
                map_number: map.number,
                elevation: trigger.elevation,
                hex: trigger.hex,
                reference: trigger.reference.clone(),
                shape: trigger.shape.clone(),
                radius: trigger.radius,
                editor_id: source.and_then(|s| s.editor_id.clone()),
                base: source.and_then(|s| s.base.clone()),
                base_kind: source.and_then(|s| s.base_kind.clone()),
                base_editor_id: source.and_then(|s| s.base_editor_id.clone()),
                enable_parent: source.and_then(|s| s.enable_parent.clone()),
                group: groups.get(&trigger.reference).map(group_info),
                linked_ref: source.and_then(|s| s.linked_ref.clone()),
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
                let (door, exit_grid, locked, team, ai_packet) = match &object.state {
                    ObjectState::Critter(state) => {
                        (false, false, false, Some(state.team), Some(state.ai))
                    }
                    ObjectState::Other { word, detail } => (
                        matches!(detail, ObjectDetail::Door(_)),
                        matches!(detail, ObjectDetail::ExitGrid { .. }),
                        match detail {
                            ObjectDetail::Door(state) => state.0 & LockFlags::LOCKED.0 != 0,
                            _ => object.pid >> 24 == 0 && word & LockFlags::LOCKED.0 != 0,
                        },
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
                    locked,
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

/// What the stage changed on a reference before it reached the slicer
/// (`enable_groups`, `crate::map::apply_enable_groups`): the source's own
/// flag and enable parent, which the index keeps describing.
#[derive(Debug, Clone, PartialEq)]
pub struct Original {
    /// The reference's own initially-disabled flag.
    pub initially_disabled: bool,
    /// Its enable parent.
    pub enable_parent: Option<mir::EnableParentDoc>,
}

/// What the map stage holds for one map when it writes its output.
pub struct StageMap<'a> {
    /// The map's file stem (`MapRun::stem`; `SliceSpec::map_stem` on the
    /// one-map path).
    pub stem: &'a str,
    /// Every reference of the map's cells the stage considered
    /// (`plan.maps[m].references`, the scene's references on the one-map
    /// path), disabled and deleted ones included.
    pub references: &'a [mir::Reference],
    /// Every object the builder made of the map's placements and critters
    /// (`BuiltMap::objects`, `Built::objects`).
    pub objects: &'a [map_builder::ObjectRef],
    /// The critters the builder did not place (`BuildReport::skipped_critters`).
    pub skipped_critters: &'a [map_builder::SkippedCritter],
    /// References the stage itself left out, with its words: the slicer's
    /// skips (`Sliced::skipped`, every elevation's), the people the map
    /// stage did not hand to the builder (`map::PersonSkip`) and the
    /// enable-group children it could not place. The first reason given
    /// for a reference is kept; one placed elsewhere is not listed.
    pub skipped: Vec<(String, String)>,
    /// Presence copies on this map: the copy's reference and the
    /// original's.
    pub copies: Vec<(String, String)>,
    /// The map's trigger volumes ([`trigger_spots`]).
    pub triggers: Vec<TriggerSpot>,
    /// Critter references the catalogue marks hostile.
    pub hostile: BTreeSet<String>,
    /// The PID of each key item the layer holds ([`key_pids_of`]).
    pub key_pids: BTreeMap<String, u32>,
    /// References whose flag and enable parent the stage changed for the
    /// slicer: what the source says.
    pub originals: BTreeMap<String, Original>,
    // ---- task T9: what the `sim` stage needs beyond the index
    /// Where each reference of the map's cells that is on no object of
    /// the map would stand ([`marker_spots`]); the index lists those that
    /// are markers ([`MarkerRef`], source `cell`).
    pub positions: Vec<MarkerSpot>,
    /// References outside the maps' cells that a package names and that
    /// project onto this map ([`outside_markers`]).
    pub outside: Vec<OutsideSpot>,
    /// The map is an interior.
    pub interior: bool,
    /// Per elevation, the walkable hexes as the builder left them (40,000
    /// entries by hex, passages included: `BuiltMap::walkable`).
    pub walkable: Vec<(u8, Vec<bool>)>,
    /// The map's doors that lead to another map ([`ExitFact`]).
    pub exits: Vec<ExitFact>,
    /// The PID of every item prototype the items catalogue made, by the
    /// item's base form key ([`item_pids_of`]).
    pub items: BTreeMap<String, u32>,
    /// The readability dial `world_yaw_deg` the map was built with
    /// (`mir::WorldYaw::degrees`): its hexes are those of the turned
    /// world. 0 for none.
    pub world_yaw_deg: f64,
}

/// A slicer run's skips as [`StageMap::skipped`] takes them.
pub fn slicer_skips(sliced: &map_slicer::Sliced) -> impl Iterator<Item = (String, String)> + '_ {
    sliced
        .skipped
        .iter()
        .map(|s| (s.reference.clone(), format!("slicer: {}", s.reason)))
}

/// The hostile critters of a catalogue report, by reference.
pub fn hostile_of(report: Option<&crate::catalogue::CatalogueReport>) -> BTreeSet<String> {
    report
        .map(|r| {
            r.people
                .iter()
                .filter(|p| p.side.hostile)
                .map(|p| p.reference.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The key items of a build's loot report, by the key's base form key: the
/// PID of the prototype the items catalogue made for each.
pub fn key_pids_of(loot: Option<&map_builder::LootReport>) -> BTreeMap<String, u32> {
    loot.map(|l| {
        l.items
            .iter()
            .filter(|i| i.kind == "key")
            .map(|i| (i.key.clone(), i.pid))
            .collect()
    })
    .unwrap_or_default()
}

/// Every item prototype a build's loot report made, by the item's base
/// form key (keys included): the PIDs the `sim` stage gives `AddItem` and
/// the other words that name an item that is not placed.
pub fn item_pids_of(loot: Option<&map_builder::LootReport>) -> BTreeMap<String, u32> {
    loot.map(|l| l.items.iter().map(|i| (i.key.clone(), i.pid)).collect())
        .unwrap_or_default()
}

/// Where every reference of `references` that became no object of the map
/// would stand: not deleted, with a finite position on the map, not a
/// trigger volume (the index lists those as triggers), and not in `placed`
/// (the references the builder made objects of). The hex is the map
/// stage's own projection (`Frame::hex_of_source`), the elevation the one
/// `elevation_of` names. Sorted by reference.
pub fn marker_spots(
    references: &[mir::Reference],
    placed: &BTreeSet<&str>,
    frame: &map_builder::walk::Frame,
    elevation_of: impl Fn(&mir::Reference) -> u8,
) -> Vec<MarkerSpot> {
    let mut out: Vec<MarkerSpot> = references
        .iter()
        .filter(|r| !r.flags.deleted && !r.trigger_volume && r.transform.is_finite())
        .filter(|r| !placed.contains(r.form.to_string().as_str()))
        .filter_map(|r| {
            let [x, y, _] = r.transform.position;
            Some(MarkerSpot {
                reference: r.form.to_string(),
                elevation: elevation_of(r),
                hex: frame.hex_of_source(f64::from(x), f64::from(y))?,
            })
        })
        .collect();
    out.sort_by(|a, b| a.reference.cmp(&b.reference));
    out.dedup_by(|a, b| a.reference == b.reference);
    out
}

/// The references a package names (its location or target,
/// `mir::Records::ref_referrals`) that are not among `known` (the
/// references of the layer's maps) and that stand in the worldspace
/// `worldspace` at a position the frame puts on the map: each with its
/// hex on elevation 0. Empty when the MIR cannot say (an extract older
/// than records revision 1). Sorted by reference.
///
/// The records hold source positions: each is turned by `yaw`, the map's
/// readability turn (`world_yaw_deg`), before it is projected.
pub fn outside_markers(
    records: &mir::Records,
    worldspace: &mir::FormKey,
    frame: &map_builder::walk::Frame,
    known: &BTreeSet<String>,
    yaw: &mir::WorldYaw,
) -> Vec<OutsideSpot> {
    let Ok(referrals) = records.ref_referrals() else {
        return Vec::new();
    };
    let named: BTreeSet<mir::FormKey> = referrals
        .into_iter()
        .filter(|r| {
            matches!(
                r.reason,
                mir::RefReason::PackageLocation | mir::RefReason::PackageTarget
            )
        })
        .map(|r| r.reference)
        .filter(|r| !known.contains(&r.to_string()))
        .collect();
    let mut out = Vec::new();
    for reference in named {
        let Ok(Some(row)) = records.placed_ref(&reference) else {
            continue;
        };
        if row.worldspace.as_ref() != Some(worldspace) {
            continue;
        }
        let Some([x, y, _]) = row.position else {
            continue;
        };
        if !(x.is_finite() && y.is_finite()) {
            continue;
        }
        let [x, y] = yaw.apply_xy64(x, y);
        let Some(hex) = frame.hex_of_source(x, y) else {
            continue;
        };
        out.push(OutsideSpot {
            spot: MarkerSpot {
                reference: reference.to_string(),
                elevation: 0,
                hex,
            },
            base: row.base.map(|b| b.to_string()),
            base_kind: row.base_sig,
        });
    }
    out
}

// ---- The map stage's facts for `sim`: `map-sim.json` ----------------------

/// The facts file's name, beside the placement index (not in the layer).
pub const FACTS_FILE: &str = "map-sim.json";

/// The facts file's schema version.
pub const FACTS_SCHEMA_VERSION: u32 = 1;

/// A door of a map that leads to another map: what a presence copy that
/// leaves while watched walks to (spec 7.6, `SimSlice::exits`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExitFact {
    /// The door reference's form key.
    pub door: String,
    /// The door's elevation and hex on this map.
    pub elevation: u8,
    pub hex: i32,
    /// The destination map's stem.
    pub to: String,
}

/// The walkable hexes of one elevation, as inclusive runs: `"a-b c d-e"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkRuns {
    pub elevation: u8,
    /// How many hexes the runs hold.
    pub count: u32,
    /// The runs, ascending, separated by one space; a run of one hex is
    /// the hex alone.
    pub runs: String,
}

impl WalkRuns {
    /// The runs of a 40,000-entry walkable mask.
    pub fn of_mask(elevation: u8, mask: &[bool]) -> Self {
        let mut parts: Vec<String> = Vec::new();
        let mut count = 0u32;
        let mut hex = 0usize;
        while hex < mask.len() {
            if !mask[hex] {
                hex += 1;
                continue;
            }
            let start = hex;
            while hex < mask.len() && mask[hex] {
                hex += 1;
            }
            let end = hex - 1;
            count += (end - start + 1) as u32;
            parts.push(if end == start {
                start.to_string()
            } else {
                format!("{start}-{end}")
            });
        }
        Self {
            elevation,
            count,
            runs: parts.join(" "),
        }
    }

    /// The hexes, ascending.
    pub fn hexes(&self) -> Result<Vec<u32>, PlacementError> {
        let bad = |part: &str| fail(FACTS_FILE, format!("walkable run {part:?} does not parse"));
        let mut out = Vec::with_capacity(self.count as usize);
        for part in self.runs.split_ascii_whitespace() {
            let (a, b) = match part.split_once('-') {
                Some((a, b)) => (a, b),
                None => (part, part),
            };
            let (a, b): (u32, u32) = (
                a.parse().map_err(|_| bad(part))?,
                b.parse().map_err(|_| bad(part))?,
            );
            if b < a || b >= 40_000 {
                return Err(bad(part));
            }
            out.extend(a..=b);
        }
        Ok(out)
    }
}

/// One map's facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapFacts {
    /// The map's stem.
    pub stem: String,
    /// An interior (`IsInInterior`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub interior: bool,
    /// The walkable hexes, per elevation.
    pub walkable: Vec<WalkRuns>,
    /// Doors to other maps, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<ExitFact>,
}

/// `map-sim.json`: what the map stage knows that the `sim` stage needs and
/// the placement index does not carry (walkable masks, exits, item PIDs,
/// which maps are interiors). Written by [`write_for_stage`] with the
/// index; deterministic like it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageFacts {
    /// [`FACTS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The layer's maps that the stage built, by stem.
    pub maps: Vec<MapFacts>,
    /// Item base form key to its prototype's PID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub items: BTreeMap<String, u32>,
}

impl StageFacts {
    /// The file's text: pretty JSON and a final newline.
    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Parses the file's text and checks its schema version.
    pub fn from_text(text: &str) -> Result<Self, PlacementError> {
        let facts: StageFacts = serde_json::from_str(text).map_err(|e| fail(FACTS_FILE, e))?;
        if facts.schema_version != FACTS_SCHEMA_VERSION {
            return Err(fail(
                FACTS_FILE,
                format!(
                    "schema version {} where this build reads {FACTS_SCHEMA_VERSION}; run the \
                     map stage again",
                    facts.schema_version
                ),
            ));
        }
        Ok(facts)
    }
}

/// Reads `<out>/map-sim.json`; `None` when there is none.
pub fn read_facts(out: &Path) -> Result<Option<StageFacts>, PlacementError> {
    let path = out.join(FACTS_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| fail(FACTS_FILE, e))?;
    StageFacts::from_text(&text).map(Some)
}

/// Adds the markers of `maps` to an index [`build`] made: every unplaced
/// entry of a map that is scenery, not deleted, in no enable group, and
/// has a position ([`StageMap::positions`]), then each outside reference
/// the index does not name yet. Group members are left out (the state
/// tables would take a marker for an object) and counted in a note.
fn add_markers(index: &mut PlacementIndex, maps: &[StageMap<'_>]) {
    let mut known: BTreeSet<String> = index
        .placed
        .iter()
        .filter_map(|p| p.reference.clone())
        .chain(index.unplaced.iter().map(|u| u.reference.clone()))
        .chain(index.triggers.iter().map(|t| t.reference.clone()))
        .collect();
    for map in maps {
        let Some(number) = index.map(map.stem).map(|m| m.number) else {
            continue;
        };
        let positions: BTreeMap<&str, &MarkerSpot> = map
            .positions
            .iter()
            .map(|p| (p.reference.as_str(), p))
            .collect();
        let mut grouped = 0usize;
        let mut added = Vec::new();
        for entry in index.unplaced.iter().filter(|u| u.map == map.stem) {
            if entry.kind != PlacedKind::Scenery || entry.reason == UnplacedReason::Deleted {
                continue;
            }
            let Some(spot) = positions.get(entry.reference.as_str()) else {
                continue;
            };
            if entry.group.is_some() {
                grouped += 1;
                continue;
            }
            added.push(MarkerRef {
                map: map.stem.to_owned(),
                map_number: number,
                elevation: spot.elevation,
                hex: spot.hex,
                reference: entry.reference.clone(),
                editor_id: entry.editor_id.clone(),
                base: entry.base.clone(),
                base_editor_id: entry.base_editor_id.clone(),
                source: MarkerSource::Cell,
            });
        }
        let cell = added.len();
        for outside in &map.outside {
            if !known.insert(outside.spot.reference.clone()) {
                continue;
            }
            added.push(MarkerRef {
                map: map.stem.to_owned(),
                map_number: number,
                elevation: outside.spot.elevation,
                hex: outside.spot.hex,
                reference: outside.spot.reference.clone(),
                editor_id: None,
                base: outside.base.clone(),
                base_editor_id: None,
                source: MarkerSource::Outside,
            });
        }
        if !added.is_empty() || grouped > 0 {
            index.notes.push(format!(
                "{}: {} markers ({cell} of its cells, {} outside them named by a package); \
                 {grouped} unplaced group members left out of the markers",
                map.stem,
                added.len(),
                added.len() - cell
            ));
        }
        index.markers.extend(added);
    }
    index.sort();
}

/// The facts of `maps` (`map-sim.json`).
fn stage_facts(maps: &[StageMap<'_>]) -> StageFacts {
    let mut facts = StageFacts {
        schema_version: FACTS_SCHEMA_VERSION,
        ..StageFacts::default()
    };
    for map in maps {
        let mut exits = map.exits.clone();
        exits.sort();
        exits.dedup();
        let mut walkable: Vec<WalkRuns> = map
            .walkable
            .iter()
            .map(|(elevation, mask)| WalkRuns::of_mask(*elevation, mask))
            .collect();
        walkable.sort_by_key(|w| w.elevation);
        facts.maps.push(MapFacts {
            stem: map.stem.to_owned(),
            interior: map.interior,
            walkable,
            exits,
        });
        facts
            .items
            .extend(map.items.iter().map(|(k, v)| (k.clone(), *v)));
    }
    facts.maps.sort_by(|a, b| a.stem.cmp(&b.stem));
    facts
}

/// A trigger volume's primitive: the shape and the extent along the
/// reference's local X, Y and Z, as stored (`fnv_esm::Primitive`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerBounds {
    /// `box` or `sphere`.
    pub shape: &'static str,
    /// The extents; a box reaches from `-half` to `+half` along each axis
    /// (public documentation reads the stored bounds as half-extents; the
    /// reading is not verified), a sphere's first is its radius.
    pub half: [f32; 3],
}

/// The primitive of a trigger reference (`mir::Reference::primitive`): a
/// box or a sphere. `None` when the reference has no primitive or its shape
/// is neither (a portal box or a code no layout is known for is not a
/// trigger), so the trigger is listed with its hex and no radius.
pub fn trigger_bounds(reference: &mir::Reference) -> Option<TriggerBounds> {
    let primitive = reference.primitive?;
    let shape = match primitive.shape {
        mir::PrimitiveShape::Box => "box",
        mir::PrimitiveShape::Sphere => "sphere",
        mir::PrimitiveShape::PortalBox | mir::PrimitiveShape::Other(_) => return None,
    };
    Some(TriggerBounds {
        shape,
        half: primitive.bounds,
    })
}

/// The hex radius around the hex under a volume's centre that covers its
/// footprint: the largest hex distance from the centre's hex to a corner
/// of the box (rotated by `yaw`, scaled), or to the four points a sphere's
/// radius reaches. A corner that falls off the map is pulled in along its
/// line to the centre until it is on the map (bisection to the map's edge),
/// since nothing can stand beyond it. `None` when the centre is off the map.
pub fn project_trigger(
    frame: &map_builder::walk::Frame,
    centre: [f32; 2],
    yaw: f32,
    scale: f32,
    bounds: TriggerBounds,
) -> Option<u32> {
    let from = frame.hex_of_source(f64::from(centre[0]), f64::from(centre[1]))?;
    let (sin, cos) = (f64::from(yaw).sin(), f64::from(yaw).cos());
    let [hx, hy, _] = bounds.half.map(|h| f64::from(h) * f64::from(scale));
    let corners: Vec<[f64; 2]> = if bounds.shape == "sphere" {
        vec![[hx, 0.0], [-hx, 0.0], [0.0, hx], [0.0, -hx]]
    } else {
        vec![[hx, hy], [hx, -hy], [-hx, hy], [-hx, -hy]]
    };
    let mut radius = 0;
    for [dx, dy] in corners {
        let at = |t: f64| {
            let (dx, dy) = (dx * t, dy * t);
            let x = f64::from(centre[0]) + dx * cos - dy * sin;
            let y = f64::from(centre[1]) + dx * sin + dy * cos;
            frame.hex_of_source(x, y)
        };
        let to = match at(1.0) {
            Some(hex) => hex,
            None => {
                // Off the map: the farthest point of the line that is on it.
                let (mut on, mut off) = (0.0_f64, 1.0_f64);
                for _ in 0..32 {
                    let middle = (on + off) / 2.0;
                    if at(middle).is_some() {
                        on = middle;
                    } else {
                        off = middle;
                    }
                }
                at(on)?
            }
        };
        radius = radius.max(vault_formats::HexGrid::lattice_distance(from, to).ok()?);
    }
    u32::try_from(radius).ok()
}

/// The trigger volumes among `references` as the index lists them: the
/// hex under each one's centre, on the elevation `elevation_of` names,
/// with the radius and shape when [`trigger_bounds`] knows them. A deleted
/// reference or one off the map is left out.
pub fn trigger_spots(
    references: &[mir::Reference],
    frame: &map_builder::walk::Frame,
    elevation_of: impl Fn(&mir::Reference) -> u8,
) -> Vec<TriggerSpot> {
    let mut out: Vec<TriggerSpot> = references
        .iter()
        .filter(|r| r.trigger_volume && !r.flags.deleted && r.transform.is_finite())
        .filter_map(|r| {
            let [x, y, _] = r.transform.position;
            let hex = frame.hex_of_source(f64::from(x), f64::from(y))?;
            let bounds = trigger_bounds(r);
            Some(TriggerSpot {
                reference: r.form.to_string(),
                elevation: elevation_of(r),
                hex,
                shape: bounds.map(|b| b.shape.to_owned()),
                radius: bounds.and_then(|b| {
                    project_trigger(frame, [x, y], r.transform.rotation[2], r.transform.scale, b)
                }),
            })
        })
        .collect();
    out.sort_by(|a, b| a.reference.cmp(&b.reference));
    out.dedup_by(|a, b| a.reference == b.reference);
    out
}

impl<'a> StageMap<'a> {
    /// One map of a layer, from its built map.
    pub fn of_layer(
        stem: &'a str,
        references: &'a [mir::Reference],
        built: &'a map_builder::BuiltMap,
        skipped: Vec<(String, String)>,
    ) -> Self {
        StageMap {
            stem,
            references,
            objects: &built.objects,
            skipped_critters: &built.report.skipped_critters,
            skipped,
            copies: Vec::new(),
            triggers: Vec::new(),
            hostile: BTreeSet::new(),
            key_pids: BTreeMap::new(),
            originals: BTreeMap::new(),
            positions: Vec::new(),
            outside: Vec::new(),
            interior: false,
            walkable: Vec::new(),
            exits: Vec::new(),
            items: BTreeMap::new(),
            world_yaw_deg: 0.0,
        }
    }
}

fn source_of(reference: &mir::Reference, originals: &BTreeMap<String, Original>) -> SourceRef {
    let key = reference.form.to_string();
    let (initially_disabled, parent) = match originals.get(&key) {
        Some(o) => (o.initially_disabled, o.enable_parent.as_ref()),
        None => (
            reference.flags.initially_disabled,
            reference.enable_parent.as_ref(),
        ),
    };
    SourceRef {
        reference: key,
        editor_id: reference.editor_id.clone(),
        base: Some(reference.base.to_string()),
        base_kind: reference.base_kind.clone(),
        base_editor_id: reference.base_editor_id.clone(),
        critter: !matches!(reference.kind, mir::RefKind::Object),
        enable_parent: parent.map(|p| EnableParent {
            reference: p.reference.to_string(),
            opposite: p.opposite,
        }),
        initially_disabled,
        deleted: reference.flags.deleted,
        lock: reference
            .lock
            .as_ref()
            .map(|l| (l.level, l.key.as_ref().map(ToString::to_string))),
        linked_ref: reference.linked_ref.as_ref().map(ToString::to_string),
    }
}

/// One map's inputs, owned until [`build`] borrows them.
struct Owned {
    stem: String,
    number: u32,
    references: Vec<SourceRef>,
    objects: Vec<ObjectSpot>,
    skipped: Vec<(String, String)>,
    written: Vec<WrittenObject>,
    copies: Vec<(String, String)>,
    triggers: Vec<TriggerSpot>,
    hostile: BTreeSet<String>,
    key_pids: BTreeMap<String, u32>,
}

/// The map stage's one call, after it has written the layer to
/// `layer_dir`: joins its references and the builder's exact object map
/// with the objects of the written maps and writes
/// `<out>/map-placements.json`.
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
        let mut skipped: Vec<(String, String)> = map
            .skipped_critters
            .iter()
            .map(|s| (s.reference.clone(), s.reason.clone()))
            .collect();
        skipped.extend(map.skipped.iter().cloned());
        owned.push(Owned {
            stem: map.stem.to_owned(),
            number: found.number,
            references: map
                .references
                .iter()
                .map(|r| source_of(r, &map.originals))
                .collect(),
            objects: map
                .objects
                .iter()
                .map(|o| ObjectSpot {
                    reference: o.reference.clone(),
                    elevation: o.elevation,
                    id: o.id,
                    hex: o.hex,
                    pid: o.pid,
                })
                .collect(),
            skipped,
            written: found.objects.clone(),
            copies: map.copies.clone(),
            triggers: map.triggers.clone(),
            hostile: map.hostile.clone(),
            key_pids: map.key_pids.clone(),
        });
    }
    let inputs: Vec<MapInput<'_>> = owned
        .iter()
        .map(|o| MapInput {
            stem: &o.stem,
            number: o.number,
            references: &o.references,
            objects: &o.objects,
            skipped: &o.skipped,
            written: &o.written,
            copies: &o.copies,
            triggers: &o.triggers,
            hostile: &o.hostile,
            key_pids: &o.key_pids,
        })
        .collect();
    let mut index = build(&layer_name, game.id(), &inputs, &referenced);
    // The readability turn of each map, and the start map's in the header,
    // only when some map is turned: the file is otherwise as it was.
    for map in maps.iter().filter(|m| m.world_yaw_deg != 0.0) {
        for entry in index.maps.iter_mut().filter(|e| e.stem == map.stem) {
            entry.world_yaw_deg = Some(map.world_yaw_deg);
        }
    }
    if maps.iter().any(|m| m.world_yaw_deg != 0.0) {
        index.world_yaw_deg = maps.first().map(|m| m.world_yaw_deg);
    }
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
                world_yaw_deg: None,
            });
        }
        index.notes.push(format!(
            "{} stub map{} place no source reference",
            stubs.len(),
            if stubs.len() == 1 { "" } else { "s" }
        ));
        index.sort();
    }
    add_markers(&mut index, maps);
    let facts = stage_facts(maps);
    let facts_path = out.join(FACTS_FILE);
    std::fs::write(&facts_path, facts.to_text()).map_err(|e| fail(FACTS_FILE, e))?;
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

/// Whether `<out>/map-placements.json` is there and of this schema, and
/// `<out>/map-sim.json` beside it: the map stage reuses its output only
/// then (a run from before task T9 has no facts file and no markers, and
/// is made again).
pub fn is_current(out: &Path) -> bool {
    matches!(read(out), Ok(Some(index)) if index.origin == Origin::Stage)
        && matches!(read_facts(out), Ok(Some(_)))
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
        world_yaw_deg: None,
        maps: Vec::new(),
        placed: Vec::new(),
        unplaced: Vec::new(),
        triggers: Vec::new(),
        markers: Vec::new(),
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
            world_yaw_deg: None,
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
                reference,
                hidden: object.hidden,
                team: object.team,
                ai_packet: object.ai_packet,
                ..PlacedRef::default()
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
