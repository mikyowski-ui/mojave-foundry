//! The build (spec 3.9): from the MIR records of the slice's placed set,
//! the placements and the configuration, to a [`Module`].
//!
//! [`build`] reads the MIR ([`crate::source::load`]) and calls
//! [`build_module`], which is a pure function of its inputs: every phase
//! walks its inputs in form or [`crate::ir::SimId`] order, so the module
//! (texts, ids, every map) is the same whatever order the records came in.
//!
//! The phases, in order: names; the converter labels; the object scripts of
//! the slice's bases; packages; INFOs and their result scripts; terminals;
//! quests (those the slice's code and dialogue name, then
//! [`SimConfig::quest_depth`] more levels of the quests their scripts
//! name); topics; notes; globals; references; enable groups; actors.
//!
//! What the build does not do: per-speaker folding, skill checks, the
//! dialogue's node model (task T5, `dialogue`), quest state globals and
//! Pip-Boy rows (T5, `quest`), package runners (T6), lowering (T8).

mod records;
mod scripts;
mod world;

use std::collections::{BTreeMap, BTreeSet};

use crate::diag::Diag;
use crate::error::TranspileError;
use crate::ir::{FormKey, Module, PlacedObject, RefUses, SimConfig, SimSlice};
use crate::obscript::names::NameTable;
use crate::source::{self, SimSource};
use crate::texts::TextTable;

/// Reads the slice's records from a MIR and builds the module.
///
/// # Errors
///
/// When the MIR cannot be read, or the slice is inconsistent.
pub fn build(
    mir: &mir::Mir,
    slice: &SimSlice,
    config: &SimConfig,
) -> Result<Module, TranspileError> {
    let source = source::load(mir, slice)?;
    build_module(&source, slice, config)
}

/// Builds the module from what [`build`] read.
///
/// # Errors
///
/// When the slice is inconsistent: one reference placed twice on one map.
pub fn build_module(
    source: &SimSource,
    slice: &SimSlice,
    config: &SimConfig,
) -> Result<Module, TranspileError> {
    let mut placement = slice.placement.clone();
    placement.sort();
    let mut placed: BTreeMap<FormKey, Vec<PlacedObject>> = BTreeMap::new();
    for object in &placement.objects {
        if let Some(reference) = &object.reference {
            let list = placed.entry(reference.clone()).or_default();
            if list.iter().any(|o| o.map == object.map) {
                return Err(TranspileError::Slice(format!(
                    "reference {reference} is placed twice on map {}",
                    object.map
                )));
            }
            list.push(object.clone());
        }
    }
    let mut builder = Builder {
        source,
        config,
        names: source.name_table(),
        texts: TextTable::with_labels(),
        module: Module::default(),
        placed,
        uses: Uses::default(),
    };
    builder.run();
    Ok(builder.finish())
}

/// What the compiled code and conditions name, gathered across phases.
#[derive(Debug, Default)]
struct Uses {
    /// References and why.
    refs: BTreeMap<FormKey, RefUses>,
    /// Globals.
    globals: BTreeSet<FormKey>,
    /// Quests.
    quests: BTreeSet<FormKey>,
    /// Notes.
    notes: BTreeSet<FormKey>,
    /// References an `Enable` or `Disable` runs on.
    enabled: BTreeSet<FormKey>,
}

impl Uses {
    fn mark(&mut self, reference: &FormKey, why: RefUses) {
        self.refs.entry(reference.clone()).or_default().insert(why);
    }
}

struct Builder<'a> {
    source: &'a SimSource,
    config: &'a SimConfig,
    names: NameTable,
    texts: TextTable,
    module: Module,
    /// The slice's references and their placements, sorted.
    placed: BTreeMap<FormKey, Vec<PlacedObject>>,
    uses: Uses,
}

impl Builder<'_> {
    fn run(&mut self) {
        self.object_scripts();
        self.packages();
        self.infos();
        self.terminals();
        self.quests();
        self.topics();
        self.notes();
        self.globals();
        self.refs();
        self.groups();
        self.actors();
    }

    fn diag(&mut self, diag: Diag) {
        self.module.diagnostics.push(diag);
    }

    fn finish(mut self) -> Module {
        self.module.texts = std::mem::take(&mut self.texts).into_texts();
        self.module.diagnostics.sort();
        self.module.diagnostics.dedup();
        self.module
    }
}
