//! Numbering and namespaces (spec 10.2, contracts 11.4): every engine
//! number the layer allocates, derived from stable keys only.
//!
//! - **Programs**: one per [`ProgramKey`]; stems are a prefix (`mm` map
//!   scripts, `md` actor programs, `mo` object and note programs, `mt`
//!   terminals, `mfquest` the dispatcher) and six base-36 digits, numbered
//!   from 1 in key order within the prefix. Rows are the base list's count
//!   plus the rank of the stem: `emit::append_script_rows` numbers rows by
//!   ascending key, and the key is the stem.
//! - **Globals**: the base count `G`, then the module's globals (the
//!   middle passes allocate every one the lowering reads: quest state,
//!   variables, groups, notes, once-flags, actor globals), ordered by
//!   `(purpose, SimId)`, the order of
//!   [`crate::ir::Module::globals_in_allocation_order`].
//! - **Reference slots** from 1 in the order of `Module::refs`.
//! - **Holodisks** `k` from 1 in note `SimId` order: `pipboy.msg` title
//!   `100000 + k`, text window `200000 + 500 k`.
//! - **Quests**: `map.msg` heading `100000 + q` per journal group,
//!   `quests.msg` description `100000 + o` per journal row, both from 1 in
//!   `SimId` order.
//!
//! Nothing here depends on the order the input records came in: every
//! list is a `BTreeMap` or sorted before it is numbered.

use std::collections::{BTreeMap, BTreeSet};

use crate::diag::{Category, Diag};
use crate::ir::{
    ActorKind, BaseFiles, Call, Cond, Expr, FormKey, Global, GlobalPurpose, Module, PlacedKind,
    PlacedObject, RefExpr, SimId, SimKind, Stmt, VarRef,
};
use crate::lowered::LowerFacts;

/// What a program is made for. The stem prefix comes from the variant;
/// the ordinal from the key's order within the prefix.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProgramKey {
    /// A map's script, by map stem.
    Map(String),
    /// An actor base's program (dialogue, script, packages).
    Critter(FormKey),
    /// A non-actor base's program: its script and, per lock, its lock.
    Object {
        /// The base form.
        base: FormKey,
        /// The lock (level, key), when the program carries one.
        lock: Option<(u8, Option<FormKey>)>,
    },
    /// A note item's program (picking it up adds the note).
    Note(SimId),
    /// A terminal's program.
    Terminal(SimId),
    /// The quest dispatcher, `mfquest` (spec 5.6).
    Dispatcher,
}

impl ProgramKey {
    /// The stem prefix.
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Map(_) => "mm",
            Self::Critter(_) => "md",
            Self::Object { .. } | Self::Note(_) => "mo",
            Self::Terminal(_) => "mt",
            Self::Dispatcher => "mfquest",
        }
    }

    /// The kind name of the plan.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Map(_) => "map",
            Self::Critter(_) => "critter",
            Self::Object { .. } => "object",
            Self::Note(_) => "note",
            Self::Terminal(_) => "terminal",
            Self::Dispatcher => "dispatcher",
        }
    }

    /// The source form or map the program is for.
    pub fn owner(&self) -> String {
        match self {
            Self::Map(stem) => stem.clone(),
            Self::Critter(base) | Self::Object { base, .. } => base.to_string(),
            Self::Note(id) | Self::Terminal(id) => id.form.to_string(),
            Self::Dispatcher => "quests".to_owned(),
        }
    }
}

/// One program's numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramNumber {
    /// What it is for.
    pub key: ProgramKey,
    /// The stem.
    pub stem: String,
    /// The `scripts.lst` row (0-based script index).
    pub row: u32,
}

impl ProgramNumber {
    /// The dialogue message list of the program (row + 1, E11).
    pub fn list(&self) -> i32 {
        self.row as i32 + 1
    }
}

/// One campaign global the layer adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalNumber {
    /// Its index in the campaign array.
    pub index: u32,
    /// The global.
    pub global: Global,
    /// The `vault13.gam` name: `MF_<KIND>_<NAME>`.
    pub gam_name: String,
}

/// A reference slot (spec 5.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefSlot {
    /// k, from 1.
    pub k: u32,
    /// The placement, when the reference is placed.
    pub placed: Option<PlacedObject>,
    /// A critter: it stores itself.
    pub mobile: bool,
}

impl RefSlot {
    /// The exported variable's name.
    pub fn name(&self) -> String {
        format!("r{}", self.k)
    }
}

/// Every number of the layer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Numbering {
    /// Rows in the base `scripts.lst`.
    pub base_rows: u32,
    /// Globals the base `vault13.gam` allocates.
    pub base_globals: u32,
    /// Programs in row order.
    pub programs: Vec<ProgramNumber>,
    /// Program key to its index in [`Self::programs`].
    pub by_key: BTreeMap<ProgramKey, usize>,
    /// The layer's globals by id.
    pub globals: BTreeMap<SimId, GlobalNumber>,
    /// Reference slots.
    pub slots: BTreeMap<FormKey, RefSlot>,
    /// Map stem to number.
    pub maps: BTreeMap<String, u32>,
    /// Note to k.
    pub notes: BTreeMap<SimId, u32>,
    /// Quest to its `map.msg` heading id.
    pub quest_headings: BTreeMap<SimId, i32>,
    /// Objective to its `quests.msg` description id.
    pub objective_messages: BTreeMap<SimId, i32>,
    /// Base form to PID: the facts' table, then the placement's.
    pub pids: BTreeMap<FormKey, u32>,
}

/// The first id of the shared message ranges (spec 10.2, 9.2).
pub const SHARED_FIRST: i32 = 100_000;
/// The first holodisk text id; note k's window starts at `+ 500 k`.
pub const HOLODISK_TEXT: i32 = 200_000;

/// The ids of the globals the lowering reads, as the middle passes
/// allocate them (`ir::once_sub`, `ir::actor_sub`, spec 4.5, 5.4, 6.1).
pub mod derived {
    use crate::ir::{FormKey, SimId, SimKind, actor_sub, once_sub};

    /// An INFO's say-once flag (1 once said).
    pub fn say_once(info: &SimId) -> SimId {
        info.with_sub(once_sub::SAY_ONCE)
    }
    /// An INFO's say-once-a-day global: the game day said, -1 before.
    pub fn say_day(info: &SimId) -> SimId {
        info.with_sub(once_sub::SAY_DAY)
    }
    /// A challenge INFO's "attempted" flag.
    pub fn attempted(info: &SimId) -> SimId {
        info.with_sub(once_sub::ATTEMPTED)
    }
    /// A challenge INFO's "passed" flag.
    pub fn passed(info: &SimId) -> SimId {
        info.with_sub(once_sub::PASSED)
    }
    /// A topic given by `AddTopic`.
    pub fn topic_given(topic: &FormKey) -> SimId {
        SimId::new(SimKind::Topic, topic.clone(), once_sub::GIVEN)
    }
    /// An actor base's kill counter (`GetDeadCount`).
    pub fn dead_count(base: &FormKey) -> SimId {
        SimId::new(SimKind::Actor, base.clone(), actor_sub::KILLS)
    }
    /// A reference's dead flag.
    pub fn dead(reference: &FormKey) -> SimId {
        SimId::new(SimKind::Ref, reference.clone(), actor_sub::DEAD)
    }
    /// A hoisted object-script variable, keyed by its owner reference.
    pub fn hoisted(owner: &FormKey, index: u32) -> SimId {
        SimId::new(SimKind::ScriptVar, owner.clone(), index)
    }
    /// A quest variable.
    pub fn quest_var(quest: &SimId, index: u32) -> SimId {
        SimId::new(SimKind::QuestVar, quest.form.clone(), index)
    }
    /// The karma shadow (spec 6.4).
    pub fn karma() -> SimId {
        SimId::karma()
    }
}

/// Visits every statement list, expression and condition of the module.
pub(crate) struct Walk<'a> {
    pub calls: Vec<&'a Call>,
    pub vars: Vec<&'a VarRef>,
    pub sets: Vec<(&'a VarRef, &'a Expr)>,
}

impl<'a> Walk<'a> {
    pub fn module(module: &'a Module) -> Self {
        let mut walk = Self {
            calls: Vec::new(),
            vars: Vec::new(),
            sets: Vec::new(),
        };
        for script in module.scripts.values() {
            for block in &script.blocks {
                walk.stmts(&block.body);
            }
        }
        for info in module.infos.values() {
            walk.cond(&info.condition);
            for block in info.begin.iter().chain(&info.end) {
                walk.stmts(&block.body);
            }
        }
        for package in module.packages.values() {
            walk.cond(&package.condition);
        }
        for quest in module.quests.values() {
            for stage in &quest.stages {
                for entry in &stage.entries {
                    walk.cond(&entry.condition);
                }
            }
        }
        for terminal in module.terminals.values() {
            for item in &terminal.items {
                walk.cond(&item.condition);
            }
        }
        walk
    }

    fn stmts(&mut self, stmts: &'a [Stmt]) {
        for stmt in stmts {
            match stmt {
                Stmt::Set(var, value) => {
                    self.vars.push(var);
                    self.sets.push((var, value));
                    self.expr(value);
                }
                Stmt::If { arms, otherwise } => {
                    for (cond, body) in arms {
                        self.expr(cond);
                        self.stmts(body);
                    }
                    self.stmts(otherwise);
                }
                Stmt::Call(call) => self.call(call),
                Stmt::Return => {}
            }
        }
    }

    fn call(&mut self, call: &'a Call) {
        self.calls.push(call);
        if let Some(on) = &call.on {
            self.reference(on);
        }
        for arg in &call.args {
            self.expr(arg);
        }
    }

    fn reference(&mut self, r: &'a RefExpr) {
        match r {
            RefExpr::Var(var) => self.vars.push(var),
            RefExpr::Linked(inner) => self.reference(inner),
            _ => {}
        }
    }

    fn expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Var(var) => self.vars.push(var),
            Expr::Ref(r) => self.reference(r),
            Expr::Call(call) => self.call(call),
            Expr::Unary(_, inner) => self.expr(inner),
            Expr::Binary(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Form(_) => {}
        }
    }

    fn cond(&mut self, cond: &'a Cond) {
        match cond {
            Cond::Not(inner) => self.cond(inner),
            Cond::All(items) | Cond::Any(items) => {
                for item in items {
                    self.cond(item);
                }
            }
            Cond::Cmp { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Cond::True | Cond::False | Cond::Fallback { .. } => {}
        }
    }
}

/// The `<KIND>` of a global's `vault13.gam` name.
fn kind_name(purpose: GlobalPurpose) -> &'static str {
    match purpose {
        GlobalPurpose::QuestState => "QUEST",
        GlobalPurpose::QuestVar => "QVAR",
        GlobalPurpose::Glob => "GLOB",
        GlobalPurpose::Group => "GRP",
        GlobalPurpose::Note => "NOTE",
        GlobalPurpose::OnceFlag => "ONCE",
        GlobalPurpose::Actor => "ACTOR",
    }
}

/// Base `scripts.lst` rows and `vault13.gam` globals.
pub fn base_counts(base: &BaseFiles) -> Result<(u32, u32), String> {
    let rows = match &base.scripts_lst {
        Some(text) => {
            vault_formats::LstTable::parse(text.as_bytes(), vault_formats::LstKind::Script)
                .map_err(|e| format!("scripts.lst: {e}"))?
                .entries()
                .len() as u32
        }
        None => 0,
    };
    let globals = match &base.vault13_gam {
        Some(text) => {
            let parsed =
                vault_formats::GamFile::parse(text.as_bytes(), vault_formats::GamSection::Campaign)
                    .map_err(|e| format!("vault13.gam: {e}"))?;
            parsed.values.len() as u32
        }
        None => 0,
    };
    Ok((rows, globals))
}

fn base36(mut value: u32) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = [b'0'; 6];
    for slot in out.iter_mut().rev() {
        *slot = digits[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Which actor bases get a program: placed, not a leveled list, and with a
/// script, packages or a dialogue.
pub fn actor_programs(module: &Module, facts: &LowerFacts) -> BTreeSet<FormKey> {
    let placed: BTreeSet<FormKey> = facts
        .placement
        .objects
        .iter()
        .filter(|o| o.kind == PlacedKind::Critter)
        .filter_map(|o| o.base.clone())
        .collect();
    module
        .actors
        .values()
        .filter(|a| a.kind != ActorKind::Leveled)
        .filter(|a| placed.contains(&a.base) || !a.placements.is_empty())
        .filter(|a| {
            a.script.is_some()
                || !a.packages.is_empty()
                || module
                    .dialogues
                    .get(&a.base)
                    .is_some_and(|d| !d.nodes.is_empty())
                || module.runs.get(&a.base).is_some_and(|r| r.combat.hostile)
        })
        .map(|a| a.base.clone())
        .collect()
}

/// The lock of a placed object: the module's, else the index's.
pub fn lock_of(module: &Module, object: &PlacedObject) -> Option<(u8, Option<FormKey>)> {
    if let Some(lock) = object.reference.as_ref().and_then(|r| module.locks.get(r)) {
        return Some((lock.level, lock.key.clone()));
    }
    object
        .lock
        .as_ref()
        .filter(|l| l.locked)
        .map(|l| (l.level, l.key.clone()))
}

/// The object program key of a placed non-critter object, if it gets one.
pub fn object_key(
    module: &Module,
    facts: &LowerFacts,
    object: &PlacedObject,
) -> Option<ProgramKey> {
    let base = object.base.as_ref()?;
    if object.kind == PlacedKind::Critter {
        return None;
    }
    let terminal = SimId::of(SimKind::Terminal, base);
    if module.terminal_programs.contains_key(&terminal) || module.terminals.contains_key(&terminal)
    {
        return Some(ProgramKey::Terminal(terminal));
    }
    let note = SimId::of(SimKind::Note, base);
    if module.notes.contains_key(&note) {
        return Some(ProgramKey::Note(note));
    }
    let lock = lock_of(module, object);
    let scripted = facts
        .base_scripts
        .get(base)
        .is_some_and(|s| module.scripts.contains_key(s));
    (scripted || lock.is_some()).then(|| ProgramKey::Object {
        base: base.clone(),
        lock,
    })
}

/// Allocates every number.
pub fn number(
    module: &Module,
    facts: &LowerFacts,
    base: &BaseFiles,
    diags: &mut Vec<Diag>,
) -> Result<Numbering, String> {
    let (base_rows, base_globals) = base_counts(base)?;
    let mut placement = facts.placement.clone();
    placement.sort();

    // Maps and slots.
    let mut maps: BTreeMap<String, u32> = BTreeMap::new();
    for object in &placement.objects {
        maps.entry(object.map.clone()).or_insert(object.map_number);
    }
    let mut slots = BTreeMap::new();
    for (k, (form, entry)) in module.refs.iter().enumerate() {
        let placed = placement
            .of_reference(form)
            .find(|o| !o.presence)
            .or_else(|| placement.of_reference(form).next())
            .cloned();
        slots.insert(
            form.clone(),
            RefSlot {
                k: k as u32 + 1,
                mobile: entry.mobile
                    || placed
                        .as_ref()
                        .is_some_and(|p| p.kind == PlacedKind::Critter),
                placed,
            },
        );
    }

    // PIDs.
    let mut pids = facts.pids.clone();
    for object in &placement.objects {
        if let (Some(base), Some(pid)) = (&object.base, object.pid) {
            pids.entry(base.clone()).or_insert(pid);
        }
    }

    // Programs.
    let mut keys: BTreeSet<ProgramKey> = BTreeSet::new();
    for base in actor_programs(module, facts) {
        keys.insert(ProgramKey::Critter(base));
    }
    for object in &placement.objects {
        if let Some(key) = object_key(module, facts, object) {
            keys.insert(key);
        }
    }
    let map_needs: BTreeSet<String> = slots
        .values()
        .filter_map(|s| s.placed.as_ref().map(|p| p.map.clone()))
        .chain(
            module
                .state
                .iter()
                .filter(|(_, rows)| !rows.is_empty())
                .map(|(map, _)| map.clone()),
        )
        .filter(|map| maps.contains_key(map))
        .collect();
    for map in map_needs {
        keys.insert(ProgramKey::Map(map));
    }
    if !module.quests.is_empty() {
        keys.insert(ProgramKey::Dispatcher);
    }
    let mut ordinals: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut stems: Vec<(String, ProgramKey)> = keys
        .into_iter()
        .map(|key| {
            let prefix = key.prefix();
            let stem = if key == ProgramKey::Dispatcher {
                prefix.to_owned()
            } else {
                let n = ordinals.entry(prefix).or_insert(0);
                *n += 1;
                format!("{prefix}{}", base36(*n))
            };
            (stem, key)
        })
        .collect();
    stems.sort();
    let mut programs = Vec::new();
    let mut by_key = BTreeMap::new();
    for (rank, (stem, key)) in stems.into_iter().enumerate() {
        by_key.insert(key.clone(), programs.len());
        programs.push(ProgramNumber {
            key,
            stem,
            row: base_rows + rank as u32,
        });
    }

    // Globals: the module's, in allocation order.
    let mut globals = BTreeMap::new();
    for (rank, g) in module.globals_in_allocation_order().into_iter().enumerate() {
        let mut gam_name = format!("MF_{}_{}", kind_name(g.purpose), g.name);
        gam_name.truncate(200);
        globals.insert(
            g.id.clone(),
            GlobalNumber {
                index: base_globals + rank as u32,
                global: g.clone(),
                gam_name,
            },
        );
    }

    // Holodisks and quests.
    let disks: BTreeSet<SimId> = if module.holodisks.is_empty() {
        module.notes.keys().cloned().collect()
    } else {
        module.holodisks.keys().cloned().collect()
    };
    let notes = disks
        .into_iter()
        .enumerate()
        .map(|(k, id)| (id, k as u32 + 1))
        .collect();
    let mut quest_headings = BTreeMap::new();
    let mut objective_messages = BTreeMap::new();
    let mut objective_rank = 0;
    for (quest, journal) in &module.journal {
        let q = quest_headings.len() as i32 + 1;
        quest_headings.insert(quest.clone(), SHARED_FIRST + q);
        for row in &journal.rows {
            objective_rank += 1;
            objective_messages.insert(row.objective.clone(), SHARED_FIRST + objective_rank);
        }
    }

    for (form, slot) in &slots {
        if slot.placed.is_none() {
            diags.push(
                Diag::note(
                    Category::RefOutsideSlice,
                    format!("reference {form} has slot {} but no placement", slot.k),
                )
                .about(form.as_str()),
            );
        }
    }

    Ok(Numbering {
        base_rows,
        base_globals,
        programs,
        by_key,
        globals,
        slots,
        maps,
        notes,
        quest_headings,
        objective_messages,
        pids,
    })
}

impl Numbering {
    /// A global's index.
    pub fn global(&self, id: &SimId) -> Option<u32> {
        self.globals.get(id).map(|g| g.index)
    }

    /// A program by key.
    pub fn program(&self, key: &ProgramKey) -> Option<&ProgramNumber> {
        self.by_key.get(key).map(|&i| &self.programs[i])
    }

    /// A map's number.
    pub fn map_number(&self, stem: &str) -> Option<u32> {
        self.maps.get(stem).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_are_six_base36_digits() {
        assert_eq!(base36(1), "000001");
        assert_eq!(base36(36), "000010");
        assert_eq!(base36(36 * 36 + 35), "00010z");
    }
}
