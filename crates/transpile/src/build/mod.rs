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
//! Then the middle passes, each a module of its own taking a [`Pass`]:
//! [`crate::quest`] (quest state, variables, Pip-Boy rows, ordering
//! reports), [`crate::state`] (groups from the placement, the per-map
//! tables, actor globals), [`crate::terminals`] (terminal programs, locks,
//! holodisks, messages), [`crate::dialogue`] (per-speaker dialogues, checks,
//! once-flags) and [`crate::packages`] (runners, teams, presence).
//! Lowering is task T8's.

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
    for object in &mut placement.objects {
        // A presence copy's reference may still carry the index's
        // `@<stem>` suffix.
        if let Some(reference) = &mut object.reference
            && let Some((form, _)) = reference.0.split_once('@')
        {
            *reference = FormKey(form.to_owned());
            object.presence = true;
        }
    }
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
    let Builder {
        names,
        mut texts,
        mut module,
        placed,
        uses,
        ..
    } = builder;
    let mut pass = Pass {
        source,
        slice,
        config,
        names: &names,
        placed: &placed,
        messages: &uses.messages,
        texts: &mut texts,
        module: &mut module,
    };
    let mut setting_diags = Vec::new();
    pass.module.settings =
        crate::dialogue::checks::read_settings(&source.settings, &mut setting_diags);
    pass.module.diagnostics.extend(setting_diags);
    crate::quest::compile(&mut pass);
    crate::state::compile(&mut pass);
    crate::terminals::compile(&mut pass);
    crate::dialogue::compile(&mut pass);
    crate::packages::compile(&mut pass);
    module.texts = texts.into_texts();
    module.diagnostics.sort();
    module.diagnostics.dedup();
    Ok(module)
}

/// What a middle pass reads and writes: the source, the slice, the names,
/// the placements by reference, the text table and the module the front
/// end built.
pub struct Pass<'a> {
    /// What the build read.
    pub source: &'a SimSource,
    /// The slice, as given.
    pub slice: &'a SimSlice,
    /// The options.
    pub config: &'a SimConfig,
    /// The names of the plugin list.
    pub names: &'a NameTable,
    /// The slice's references and their placements (one per map),
    /// sorted; presence copies are placements of the copied reference.
    pub placed: &'a BTreeMap<FormKey, Vec<PlacedObject>>,
    /// The messages (`MESG`) the compiled scripts name.
    pub messages: &'a BTreeSet<FormKey>,
    /// The text table.
    pub texts: &'a mut TextTable,
    /// The module.
    pub module: &'a mut Module,
}

impl Pass<'_> {
    /// Records a finding.
    pub fn diag(&mut self, diag: Diag) {
        self.module.diagnostics.push(diag);
    }

    /// The first placement of a reference.
    pub fn placement(&self, reference: &FormKey) -> Option<&PlacedObject> {
        self.placed.get(reference).and_then(|list| list.first())
    }

    /// A numeric game setting.
    pub fn setting(&self, name: &str) -> Option<f64> {
        self.source.settings.get(name).copied()
    }
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
    /// Messages a script shows.
    messages: BTreeSet<FormKey>,
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
}
