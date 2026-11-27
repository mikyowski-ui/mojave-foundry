//! The module and the inputs and outputs of the two entry points
//! (contracts 11.1, spec 3.9; `SimSlice`, `SimConfig`, `Placement`,
//! `BaseFiles` and `Lowered` are settled here, task T2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::dialogue::{CheckSettings, Dialogue, Info, Topic};
use super::id::{FormKey, SimId, Text};
use super::package::{ActorRun, HexAt, Package};
use super::quest::{Journal, Quest};
use super::script::Script;
use super::world::{
    Actor, EnableGroup, Global, Holodisk, Lock, MessageBox, Note, RefEntry, StateChild, Terminal,
    TerminalProgram,
};
use crate::diag::Diag;

/// Everything the front end built, in [`SimId`] order.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Module {
    /// Every text, indexed by [`super::TextId`].
    pub texts: Vec<Text>,
    /// Topics.
    pub topics: BTreeMap<SimId, Topic>,
    /// INFOs.
    pub infos: BTreeMap<SimId, Info>,
    /// Compiled scripts, records and result scripts.
    pub scripts: BTreeMap<SimId, Script>,
    /// Quests.
    pub quests: BTreeMap<SimId, Quest>,
    /// Campaign globals the layer adds.
    pub globals: BTreeMap<SimId, Global>,
    /// Packages.
    pub packages: BTreeMap<SimId, Package>,
    /// Enable-parent groups.
    pub groups: BTreeMap<SimId, EnableGroup>,
    /// Terminals.
    pub terminals: BTreeMap<SimId, Terminal>,
    /// Notes.
    pub notes: BTreeMap<SimId, Note>,
    /// The placed references something needs.
    pub refs: BTreeMap<FormKey, RefEntry>,
    /// The actor bases placed in the slice.
    pub actors: BTreeMap<FormKey, Actor>,
    /// The report's source: everything not tied to one script.
    pub diagnostics: Vec<Diag>,
    // ---- task T5 and T6 (additive; absent in older snapshots)
    /// Per-speaker dialogues ([`crate::dialogue`]), by speaker base.
    #[serde(default)]
    pub dialogues: BTreeMap<FormKey, Dialogue>,
    /// The plugin's check, hacking and lock-picking settings.
    #[serde(default)]
    pub settings: CheckSettings,
    /// Messages scripts show.
    #[serde(default)]
    pub messages: BTreeMap<SimId, MessageBox>,
    /// Pip-Boy quest groups ([`crate::quest`]), by quest.
    #[serde(default)]
    pub journal: BTreeMap<SimId, Journal>,
    /// Enable-group children by map ([`crate::state`], spec 8.3), sorted.
    #[serde(default)]
    pub state: BTreeMap<String, Vec<StateChild>>,
    /// The groups each script enables or disables (spec 8.3: the calling
    /// program applies them at once), by script.
    #[serde(default)]
    pub group_users: BTreeMap<SimId, Vec<SimId>>,
    /// Terminal programs ([`crate::terminals`]), by terminal base.
    #[serde(default)]
    pub terminal_programs: BTreeMap<SimId, TerminalProgram>,
    /// Locks of doors and containers, by reference.
    #[serde(default)]
    pub locks: BTreeMap<FormKey, Lock>,
    /// Holodisks, by note.
    #[serde(default)]
    pub holodisks: BTreeMap<SimId, Holodisk>,
    /// How actors run their packages ([`crate::packages`]), by base.
    #[serde(default)]
    pub runs: BTreeMap<FormKey, ActorRun>,
    /// The team factions in team order (the lowering numbers them from
    /// 100, spec 7.5).
    #[serde(default)]
    pub teams: Vec<FormKey>,
}

impl Module {
    /// The globals in allocation order (spec 10.2): by purpose, then by id.
    pub fn globals_in_allocation_order(&self) -> Vec<&Global> {
        let mut out: Vec<&Global> = self.globals.values().collect();
        out.sort_by(|a, b| (a.purpose, &a.id).cmp(&(b.purpose, &b.id)));
        out
    }

    /// The module as pretty JSON with a final newline: the snapshot form.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| unreachable!("the IR always serialises: {e}"));
        text.push('\n');
        text
    }
}

/// What a placed object is, as the placement index says (contracts 11.2,
/// `PlacedKind`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PlacedKind {
    /// A critter.
    Critter,
    /// A door.
    Door,
    /// A container.
    Container,
    /// An item.
    Item,
    /// A terminal.
    Terminal,
    /// Scenery the caller names.
    #[default]
    Scenery,
}

/// One placed reference of the slice, as the map stage placed it: a plain
/// copy of the placement index's `PlacedRef` fields that the build and the
/// lowering read (contracts 11.2). The `sim` stage fills it; `transpile`
/// does not depend on `mojave-convert`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct PlacedObject {
    /// The map's stem.
    pub map: String,
    /// The map's number in the layer.
    pub map_number: u32,
    /// Elevation 0 to 2.
    pub elevation: u8,
    /// Hex 0 to 39,999.
    pub hex: u32,
    /// What it is.
    pub kind: PlacedKind,
    /// The object's id in the written map, when one was found.
    pub object_id: Option<u32>,
    /// The object's PID, when one was found.
    pub pid: Option<u32>,
    /// The source reference; absent in a reconstructed index.
    pub reference: Option<FormKey>,
    /// The base record.
    pub base: Option<FormKey>,
    /// The written object carries the hidden flag.
    pub hidden: bool,
    // ---- schema 2 of the placement index (tasks T5, T6; the `sim` stage
    // fills them, absent means unknown)
    /// The enable group (index field `group`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<PlacedGroup>,
    /// The lock (index field `lock`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<PlacedLock>,
    /// The linked reference (index field `linked_ref`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_ref: Option<FormKey>,
    /// The catalogue marks the critter hostile (index field `hostile`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hostile: bool,
    /// A presence copy of this reference (index field `presence_of`):
    /// `reference` is then the copied actor's reference itself (the
    /// `sim` stage strips the index's `@<stem>` suffix).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub presence: bool,
    /// A marker the map stage does not place (`XMarker`,
    /// `XMarkerHeading`; index reason `skipped`): no object, `hex` and
    /// `elevation` are where it would stand. Spec 5.8: it needs no slot;
    /// package locations and `MoveTo` use its hex as a constant.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub marker: bool,
}

/// An enable group as the placement index states it (schema 2 `group`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlacedGroup {
    /// The group's root (its state).
    pub root: FormKey,
    /// Enabled when the root is disabled.
    pub opposite: bool,
    /// Starts enabled.
    pub initially_enabled: bool,
}

/// A lock as the placement index states it (schema 2 `lock`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlacedLock {
    /// The written object carries the lock bit.
    pub locked: bool,
    /// The source's lock level, as stored (255 key only).
    pub level: u8,
    /// The key's base form.
    pub key: Option<FormKey>,
    /// The converted key item's PID (index field `key_pid`); absent when
    /// the key did not convert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_pid: Option<u32>,
}

/// The walkable hexes of one map and elevation (spec 7.3): the map
/// stage's walkable mask, which sandbox candidates are sampled from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WalkMask {
    /// Elevation 0 to 2.
    pub elevation: u8,
    /// The walkable hexes, sorted.
    pub hexes: Vec<u32>,
}

/// The placed objects of the slice, sorted. [`crate::build`] reads the
/// set of references and their maps; the lowering (task T8) reads hexes,
/// PIDs and object ids.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Placement {
    /// The objects, sorted (map, elevation, object id, hex, reference).
    pub objects: Vec<PlacedObject>,
}

impl Placement {
    /// Sorts the objects into the index's order.
    pub fn sort(&mut self) {
        self.objects.sort_by(|a, b| {
            (
                &a.map,
                a.elevation,
                a.object_id.is_none(),
                a.object_id,
                a.hex,
                &a.reference,
            )
                .cmp(&(
                    &b.map,
                    b.elevation,
                    b.object_id.is_none(),
                    b.object_id,
                    b.hex,
                    &b.reference,
                ))
        });
    }

    /// The objects of one reference, in order.
    pub fn of_reference<'a>(
        &'a self,
        reference: &'a FormKey,
    ) -> impl Iterator<Item = &'a PlacedObject> {
        self.objects
            .iter()
            .filter(move |o| o.reference.as_ref() == Some(reference))
    }
}

/// The slice the build compiles: its name and what the map stage placed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SimSlice {
    /// The slice's name (recorded, nothing depends on it).
    pub name: String,
    /// The placed objects.
    pub placement: Placement,
    /// The walkable hexes, by map stem (task T6; the `sim` stage fills it
    /// from the map stage's walkable masks). Absent: sandbox locations get
    /// no candidates beyond their anchor, reported.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub walkable: BTreeMap<String, Vec<WalkMask>>,
    /// Hexes of the slice's teleport doors by destination map, for
    /// presence copies that leave while watched (spec 7.6), by map stem:
    /// `(door reference, hex, destination map stem)`. May be empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exits: BTreeMap<String, Vec<(FormKey, HexAt, String)>>,
}

/// The `[sim]` table of `foundry.toml`: build options. Every default is an
/// owner decision of spec 13 where one applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimConfig {
    /// sfall mode (owner decision 6: off). The front end only records it;
    /// code generation must not emit sfall words when it is off.
    pub sfall: bool,
    /// Hello barks (owner decision 14: off).
    pub hello_barks: bool,
    /// Follow quests named by quest scripts and stage scripts, to this
    /// many levels beyond the quests the slice's own code names.
    pub quest_depth: u32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            sfall: false,
            hello_barks: false,
            quest_depth: 1,
        }
    }
}

/// The base copies of spec 10.1, read from the layer: each file's text, or
/// `None` when the layer has none. Read by the lowering (task T8).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BaseFiles {
    /// `scripts/scripts.lst`.
    pub scripts_lst: Option<String>,
    /// `data/vault13.gam`.
    pub vault13_gam: Option<String>,
    /// `data/quests.txt`.
    pub quests_txt: Option<String>,
    /// `data/holodisk.txt`.
    pub holodisk_txt: Option<String>,
    /// `text/english/game/quests.msg`.
    pub quests_msg: Option<String>,
    /// `text/english/game/pipboy.msg`.
    pub pipboy_msg: Option<String>,
    /// `text/english/game/map.msg`.
    pub map_msg: Option<String>,
}

/// What the lowering (task T8) produces: every file of spec 10.1 that the
/// layer gains, by its path in the layer, sorted, and the report's lines.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Lowered {
    /// Layer path (`scripts/md000001.int`, `data/vault13.gam`, ...) to
    /// bytes.
    pub files: BTreeMap<String, Vec<u8>>,
    /// What the lowering found.
    pub diagnostics: Vec<Diag>,
}
