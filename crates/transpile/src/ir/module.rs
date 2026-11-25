//! The module and the inputs and outputs of the two entry points
//! (contracts 11.1, spec 3.9; `SimSlice`, `SimConfig`, `Placement`,
//! `BaseFiles` and `Lowered` are settled here, task T2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::dialogue::{Info, Topic};
use super::id::{FormKey, SimId, Text};
use super::package::Package;
use super::quest::Quest;
use super::script::Script;
use super::world::{Actor, EnableGroup, Global, Note, RefEntry, Terminal};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    Scenery,
}

/// One placed reference of the slice, as the map stage placed it: a plain
/// copy of the placement index's `PlacedRef` fields that the build and the
/// lowering read (contracts 11.2). The `sim` stage fills it; `transpile`
/// does not depend on `mojave-convert`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
