//! The generator of one program: expressions, conditions, variables,
//! references and statements (spec 5.4, 5.8, 5.10). Function calls are in
//! `calls.rs`; the per-construct procedures in `dialogue.rs`, `world.rs`,
//! `quest.rs`, `packages.rs` and `programs.rs`.

use std::collections::{BTreeMap, BTreeSet};

use super::asm::{Proc, ProgramAsm};
use super::words::{self as hw};
use crate::diag::{Category, Diag};
use crate::ir::{
    BinOp, CheckSettings, CmpOp, Cond, Expr, FormKey, Module, RefExpr, SimId, Stmt, TextId, UnOp,
    VarRef, VarTy,
};
use crate::lowered::{Counts, LowerFacts, Outcome, SectionCounts};
use crate::numbering::{Numbering, ProgramKey, derived};
use crate::texts::LABELS;

/// The value type of an expression on the stack. Objects are integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ty {
    Int,
    Float,
    Str,
}

/// The report section a count goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Dialogue,
    Scripts,
    Quests,
    State,
    Terminals,
    Packages,
}

/// A persisted reference value (spec 5.8): 0 none, -1 the player, -2 the
/// running object, k > 0 slot k.
pub(crate) type RefCode = i32;

/// System locals of every object program (spec 5.4).
pub(crate) mod locals {
    pub const SLOT: u32 = 0;
    pub const INIT: u32 = 1;
    pub const TALKED: u32 = 2;
    pub const FLAGS: u32 = 3;
    pub const LAST_GM: u32 = 4;
    /// The first runner local of a critter program (spec 7.4).
    pub const PKG: u32 = 5;
    pub const PHASE: u32 = 6;
    pub const NEXT: u32 = 8;
    pub const STUCK: u32 = 9;
    pub const ANCHOR: u32 = 10;
    pub const SCRIPTPKG: u32 = 11;
    pub const EVP: u32 = 12;
    /// A terminal's unlocked flag (spec 9.1), after the system locals.
    pub const UNLOCKED: u32 = 5;
    /// System locals of an object program.
    pub const OBJECT: u32 = 5;
    /// System and runner locals of a critter program.
    pub const CRITTER: u32 = 13;
}

/// What every program's generator shares.
pub(crate) struct Ctx<'a> {
    pub module: &'a Module,
    pub numbers: &'a Numbering,
    pub facts: &'a LowerFacts,
    pub settings: CheckSettings,
    /// Object-script variables stored in campaign globals: (script, index)
    /// to the owner reference whose global holds it.
    pub hoisted: BTreeMap<(SimId, u32), FormKey>,
    /// The codes each `ref` variable can hold, module-wide.
    pub ref_codes: BTreeMap<VarRef, BTreeSet<RefCode>>,
    /// Group of each parent reference.
    pub group_of_parent: BTreeMap<FormKey, SimId>,
    /// Groups of each child reference, with the opposite flag.
    pub groups_of_child: BTreeMap<FormKey, Vec<(SimId, bool)>>,
}

impl<'a> Ctx<'a> {
    pub fn new(module: &'a Module, numbers: &'a Numbering, facts: &'a LowerFacts) -> Self {
        let walk = crate::numbering::Walk::module(module);
        let mut hoisted = BTreeMap::new();
        for var in &walk.vars {
            if let VarRef::Hoisted {
                owner,
                script,
                index,
                ..
            } = var
            {
                hoisted
                    .entry((script.clone(), *index))
                    .or_insert_with(|| owner.clone());
            }
        }
        // Reference codes, to a fixed point through copies.
        let mut ref_codes: BTreeMap<VarRef, BTreeSet<RefCode>> = BTreeMap::new();
        let mut copies: Vec<(VarRef, VarRef)> = Vec::new();
        for (var, value) in &walk.sets {
            if var_ty(var) != Some(VarTy::Ref) {
                continue;
            }
            let entry = ref_codes.entry((*var).clone()).or_default();
            match value {
                Expr::Ref(RefExpr::Var(other)) | Expr::Var(other) => {
                    copies.push(((*var).clone(), other.clone()));
                }
                other => {
                    if let Some(code) = static_code(other, numbers) {
                        entry.insert(code);
                    }
                }
            }
        }
        loop {
            let mut changed = false;
            for (to, from) in &copies {
                let codes = ref_codes.get(from).cloned().unwrap_or_default();
                let entry = ref_codes.entry(to.clone()).or_default();
                for code in codes {
                    changed |= entry.insert(code);
                }
            }
            if !changed {
                break;
            }
        }
        let mut group_of_parent = BTreeMap::new();
        let mut groups_of_child: BTreeMap<FormKey, Vec<(SimId, bool)>> = BTreeMap::new();
        for (id, group) in &module.groups {
            group_of_parent.insert(id.form.clone(), id.clone());
            for (child, opposite) in &group.children {
                groups_of_child
                    .entry(child.clone())
                    .or_default()
                    .push((id.clone(), *opposite));
            }
        }
        let settings = module.settings.clone();
        Self {
            module,
            numbers,
            facts,
            settings,
            hoisted,
            ref_codes,
            group_of_parent,
            groups_of_child,
        }
    }
}

/// A variable's declared type.
pub(crate) fn var_ty(var: &VarRef) -> Option<VarTy> {
    match var {
        VarRef::Local { ty, .. } | VarRef::Quest { ty, .. } | VarRef::Hoisted { ty, .. } => {
            Some(*ty)
        }
        VarRef::Global(_) | VarRef::Temp(_) => None,
    }
}

/// The code of a reference value known at build time.
fn static_code(expr: &Expr, numbers: &Numbering) -> Option<RefCode> {
    match expr {
        Expr::Ref(RefExpr::Player) => Some(-1),
        Expr::Ref(RefExpr::SelfRef) => Some(-2),
        Expr::Ref(RefExpr::Placed(form)) => numbers.slots.get(form).map(|s| s.k as i32),
        Expr::Int(0) => Some(0),
        _ => None,
    }
}

/// Where a variable lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Storage {
    Global(u32),
    Local(u32),
    Frame(u32),
}

/// The context of the block being generated.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scope {
    /// The program has a running object (`self_obj` is meaningful).
    pub has_self: bool,
    /// The temporary that records whether `Activate` ran (spec 5.5).
    pub activation: Option<u32>,
    /// Where a `return` of the source goes.
    pub return_label: Option<String>,
    /// `GetSecondsPassed`: a constant, or the temporary holding it.
    pub seconds: Option<u32>,
    pub seconds_const: f32,
    /// Inside a dialogue callback (barter allowed).
    pub dialogue: bool,
    /// The report owner.
    pub owner: String,
    /// The report section of statements and conditions.
    pub section: Option<Section>,
    /// Temporaries of `VarRef::Temp`.
    pub temps: BTreeMap<u32, u32>,
}

/// One program being generated.
pub(crate) struct Gen<'c, 'a> {
    pub ctx: &'c Ctx<'a>,
    pub key: ProgramKey,
    pub asm: ProgramAsm,
    /// The program's message list (row + 1).
    pub list: i32,
    /// The program's local layout: (script, variable index) to slot.
    pub local_vars: BTreeMap<(SimId, u32), u32>,
    /// The local count of the program's row.
    pub local_count: u32,
    /// The program's message file: id to source text.
    pub msgs: BTreeMap<i32, String>,
    msg_by_text: BTreeMap<String, i32>,
    next_msg: i32,
    pub counts: Counts,
    pub diags: Vec<Diag>,
    pub scope: Scope,
    /// The actor base of a critter program.
    pub actor: Option<FormKey>,
    /// Procedures generated on demand (group appliers), by name.
    pub pending_procs: BTreeSet<String>,
    /// The speaker's dialogue, in a critter program.
    pub dialogue: Option<super::DialoguePlan>,
    /// Procedures made while another is open, added after it.
    pub polls: Vec<Proc>,
}

impl<'c, 'a> Gen<'c, 'a> {
    pub fn new(ctx: &'c Ctx<'a>, key: ProgramKey, list: i32) -> Self {
        Self {
            ctx,
            key,
            asm: ProgramAsm::default(),
            list,
            local_vars: BTreeMap::new(),
            local_count: 0,
            msgs: BTreeMap::new(),
            msg_by_text: BTreeMap::new(),
            next_msg: 1100,
            counts: Counts::default(),
            diags: Vec::new(),
            scope: Scope::default(),
            actor: None,
            pending_procs: BTreeSet::new(),
            dialogue: None,
            polls: Vec::new(),
        }
    }

    // ---- report ------------------------------------------------------------

    fn section_counts(&mut self, section: Section) -> &mut SectionCounts {
        match section {
            Section::Dialogue => &mut self.counts.dialogue,
            Section::Scripts => &mut self.counts.scripts,
            Section::Quests => &mut self.counts.quests,
            Section::State => &mut self.counts.state,
            Section::Terminals => &mut self.counts.terminals,
            Section::Packages => &mut self.counts.packages,
        }
    }

    /// Counts one item in `section`.
    pub fn count_in(&mut self, section: Section, outcome: Outcome, reason: &str) {
        let owner = self.scope.owner.clone();
        self.section_counts(section).count(outcome, reason, &owner);
    }

    /// Counts one item in the current scope's section.
    pub fn count(&mut self, outcome: Outcome, reason: &str) {
        let section = self.scope.section.unwrap_or(Section::Scripts);
        self.count_in(section, outcome, reason);
    }

    /// A lossy fallback: counted and written as a diagnostic.
    pub fn fallback(&mut self, reason: &str) {
        self.count(Outcome::Fallback, reason);
        let diag = Diag::warning(Category::DroppedFunction, reason.to_owned())
            .about(self.scope.owner.clone());
        self.diags.push(diag);
    }

    // ---- messages ----------------------------------------------------------

    /// A converter label's id (1 to 99), added to the program's file.
    pub fn label(&mut self, number: u32) -> i32 {
        if let Some((_, text)) = LABELS.iter().find(|(n, _)| *n == number) {
            self.msgs.insert(number as i32, (*text).to_owned());
        }
        number as i32
    }

    /// A message of the program's file at a chosen id.
    pub fn put_msg(&mut self, id: i32, text: String) {
        self.msgs.insert(id, text);
    }

    /// A message of the program's file with an allocated id (from 1100;
    /// equal texts share an id).
    pub fn msg(&mut self, text: &str) -> i32 {
        if let Some(&id) = self.msg_by_text.get(text) {
            return id;
        }
        let id = self.next_msg;
        self.next_msg += 1;
        if self.next_msg == 10_000 {
            // Dialogue owns 10000 up; continue past the INFO range.
            self.next_msg = 800_000;
        }
        self.msgs.insert(id, text.to_owned());
        self.msg_by_text.insert(text.to_owned(), id);
        id
    }

    /// The text of a [`TextId`].
    pub fn text(&self, id: TextId) -> String {
        self.ctx
            .module
            .texts
            .get(id.0 as usize)
            .map(|t| t.source.clone())
            .unwrap_or_default()
    }

    /// `message_str(list, id)`: pushes a string.
    pub fn message_str(&mut self, p: &mut Proc, id: i32) {
        p.push_i(self.list);
        p.push_i(id);
        p.host(hw::MESSAGE_STR);
    }

    /// `display_msg` of a label then a message.
    pub fn display(&mut self, p: &mut Proc, label: Option<u32>, id: i32) {
        match label {
            Some(label) => {
                let label = self.label(label);
                self.message_str(p, label);
                self.message_str(p, id);
                p.binary("add");
            }
            None => self.message_str(p, id),
        }
        p.host(hw::DISPLAY_MSG);
    }

    // ---- globals -----------------------------------------------------------

    /// A campaign global's index, or `None` (reported).
    pub fn global(&mut self, id: &SimId) -> Option<u32> {
        let index = self.ctx.numbers.global(id);
        if index.is_none() {
            self.fallback(&format!("no global for {id}"));
        }
        index
    }

    /// Pushes a campaign global's value.
    pub fn get_global(&mut self, p: &mut Proc, index: u32) {
        p.push_i(index as i32);
        p.host(hw::GLOBAL_VAR);
    }

    /// Sets a campaign global to the value `value` pushes.
    pub fn set_global_with(
        &mut self,
        p: &mut Proc,
        index: u32,
        value: impl FnOnce(&mut Self, &mut Proc),
    ) {
        p.push_i(index as i32);
        value(self, p);
        p.host(hw::SET_GLOBAL_VAR);
    }

    /// Sets a campaign global to a constant.
    pub fn set_global(&mut self, p: &mut Proc, index: u32, value: i32) {
        self.set_global_with(p, index, |_, p| p.push_i(value));
    }

    /// `global |= mask`.
    pub fn set_bits(&mut self, p: &mut Proc, index: u32, mask: i32) {
        self.set_global_with(p, index, |g, p| {
            g.get_global(p, index);
            p.push_i(mask);
            p.binary("bor");
        });
    }

    /// `global &= !mask`.
    pub fn clear_bits(&mut self, p: &mut Proc, index: u32, mask: i32) {
        self.set_global_with(p, index, |g, p| {
            g.get_global(p, index);
            p.push_i(!mask);
            p.binary("band");
        });
    }

    /// Pushes 1 when `global & mask` is not zero, else 0.
    pub fn test_bits(&mut self, p: &mut Proc, index: u32, mask: i32) {
        self.get_global(p, index);
        p.push_i(mask);
        p.binary("band");
        p.push_i(0);
        p.binary("ne");
    }

    /// Pushes an instance local.
    pub fn get_local(&mut self, p: &mut Proc, slot: u32) {
        p.push_i(slot as i32);
        p.host(hw::LOCAL_VAR);
    }

    /// Sets an instance local to the value `value` pushes.
    pub fn set_local_with(
        &mut self,
        p: &mut Proc,
        slot: u32,
        value: impl FnOnce(&mut Self, &mut Proc),
    ) {
        p.push_i(slot as i32);
        value(self, p);
        p.host(hw::SET_LOCAL_VAR);
    }

    /// Sets an instance local to a constant.
    pub fn set_local(&mut self, p: &mut Proc, slot: u32, value: i32) {
        self.set_local_with(p, slot, |_, p| p.push_i(value));
    }

    /// `game_time / 864000`: the game day.
    pub fn game_day(&mut self, p: &mut Proc) {
        p.host(hw::GAME_TIME);
        p.push_i(864_000);
        p.binary("div");
    }

    // ---- variables ---------------------------------------------------------

    fn storage(&mut self, var: &VarRef) -> Option<Storage> {
        let numbers = self.ctx.numbers;
        match var {
            VarRef::Local { script, index, .. } => {
                if let Some(owner) = self.ctx.hoisted.get(&(script.clone(), *index)) {
                    return numbers
                        .global(&derived::hoisted(owner, *index))
                        .map(Storage::Global);
                }
                self.local_vars
                    .get(&(script.clone(), *index))
                    .copied()
                    .map(Storage::Local)
            }
            VarRef::Quest { quest, index, .. } => numbers
                .global(&derived::quest_var(quest, *index))
                .map(Storage::Global),
            VarRef::Global(id) => numbers.global(id).map(Storage::Global),
            VarRef::Hoisted { owner, index, .. } => numbers
                .global(&derived::hoisted(owner, *index))
                .map(Storage::Global),
            VarRef::Temp(n) => {
                let slot = *self.scope.temps.get(n)?;
                Some(Storage::Frame(slot))
            }
        }
    }

    /// The stored type of a variable: a `GLOB`'s from the module.
    fn stored_ty(&self, var: &VarRef) -> VarTy {
        match var {
            VarRef::Global(id) => self
                .ctx
                .numbers
                .globals
                .get(id)
                .map_or(VarTy::Int, |g| g.global.ty),
            other => var_ty(other).unwrap_or(VarTy::Int),
        }
    }

    /// Pushes a variable's raw stored value (a reference variable's code).
    fn load_raw(&mut self, p: &mut Proc, var: &VarRef) -> Ty {
        let ty = self.stored_ty(var);
        match self.storage(var) {
            Some(Storage::Global(i)) => self.get_global(p, i),
            Some(Storage::Local(s)) => self.get_local(p, s),
            Some(Storage::Frame(j)) => {
                p.fetch_local(j);
                return if ty == VarTy::Float {
                    Ty::Float
                } else {
                    Ty::Int
                };
            }
            None => {
                if let VarRef::Temp(n) = var {
                    let slot = p.temp();
                    p.keep(slot);
                    self.scope.temps.insert(*n, slot);
                    p.fetch_local(slot);
                    return Ty::Int;
                }
                self.fallback(&format!(
                    "variable {} has no storage in this program",
                    var_name(var)
                ));
                p.push_i(0);
                return Ty::Int;
            }
        }
        if ty == VarTy::Float {
            p.push_f(1000.0);
            p.binary("div");
            Ty::Float
        } else {
            Ty::Int
        }
    }

    /// Pushes a variable's value (a reference variable's object).
    pub fn load(&mut self, p: &mut Proc, var: &VarRef) -> Ty {
        if self.stored_ty(var) == VarTy::Ref {
            self.deref(p, var);
            return Ty::Int;
        }
        self.load_raw(p, var)
    }

    /// `set var to value`.
    pub fn store(&mut self, p: &mut Proc, var: &VarRef, value: &Expr) {
        let ty = self.stored_ty(var);
        let storage = match self.storage(var) {
            Some(s) => s,
            None => {
                if let VarRef::Temp(n) = var {
                    let slot = p.temp();
                    p.keep(slot);
                    self.scope.temps.insert(*n, slot);
                    Storage::Frame(slot)
                } else {
                    self.fallback(&format!(
                        "set {}: no storage in this program; dropped",
                        var_name(var)
                    ));
                    return;
                }
            }
        };
        match storage {
            Storage::Global(i) => {
                p.push_i(i as i32);
                self.stored_value(p, ty, value);
                p.host(hw::SET_GLOBAL_VAR);
            }
            Storage::Local(s) => {
                p.push_i(s as i32);
                self.stored_value(p, ty, value);
                p.host(hw::SET_LOCAL_VAR);
            }
            Storage::Frame(j) => {
                self.expr(p, value);
                p.store_local(j);
            }
        }
        self.count(Outcome::Compiled, "");
    }

    /// Pushes `value` as a stored integer of type `ty`.
    fn stored_value(&mut self, p: &mut Proc, ty: VarTy, value: &Expr) {
        match ty {
            VarTy::Int => self.expr_int(p, value),
            VarTy::Float => {
                let t = self.expr(p, value);
                match t {
                    Ty::Int => {
                        p.push_i(1000);
                        p.binary("mul");
                    }
                    Ty::Float => {
                        p.push_f(1000.0);
                        p.binary("mul");
                        self.round(p);
                    }
                    Ty::Str => {
                        p.pop();
                        p.push_i(0);
                    }
                }
            }
            VarTy::Ref => self.ref_code(p, value),
        }
    }

    /// Rounds the float on top to the nearest integer, halves away from 0.
    pub fn round(&mut self, p: &mut Proc) {
        let t = p.temp();
        p.store_local(t);
        let positive = p.label();
        let end = p.label();
        p.branch_false(&positive, |p| {
            p.fetch_local(t);
            p.push_f(0.0);
            p.binary("lt");
        });
        p.fetch_local(t);
        p.push_f(0.5);
        p.binary("sub");
        p.unary("ftoi");
        p.jump(&end);
        p.place(&positive);
        p.fetch_local(t);
        p.push_f(0.5);
        p.binary("add");
        p.unary("ftoi");
        p.place(&end);
    }

    // ---- references --------------------------------------------------------

    /// Pushes the code of a reference value for a `ref` variable.
    fn ref_code(&mut self, p: &mut Proc, value: &Expr) {
        match value {
            Expr::Ref(RefExpr::Var(other)) | Expr::Var(other) => {
                self.load_raw(p, other);
            }
            other => match static_code(other, self.ctx.numbers) {
                Some(code) => p.push_i(code),
                None => {
                    self.fallback(
                        "a reference stored in a variable that has no slot; stored as none",
                    );
                    p.push_i(0);
                }
            },
        }
    }

    /// Pushes the object a `ref` variable's code names.
    fn deref(&mut self, p: &mut Proc, var: &VarRef) {
        let codes: Vec<RefCode> = self
            .ctx
            .ref_codes
            .get(var)
            .map(|c| c.iter().copied().filter(|c| *c != 0).collect())
            .unwrap_or_default();
        let t = p.temp();
        self.load_raw(p, var);
        p.store_local(t);
        let end = p.label();
        for code in codes {
            let next = p.label();
            p.branch_false(&next, |p| {
                p.fetch_local(t);
                p.push_i(code);
                p.binary("eq");
            });
            match code {
                -1 => p.host(hw::DUDE_OBJ),
                -2 => self.self_obj(p),
                k => {
                    let form = self
                        .ctx
                        .numbers
                        .slots
                        .iter()
                        .find(|(_, s)| s.k as i32 == k)
                        .map(|(f, _)| f.clone());
                    match form {
                        Some(form) => self.slot_object(p, &form),
                        None => p.push_i(0),
                    }
                }
            }
            p.jump(&end);
            p.place(&next);
        }
        p.push_i(0);
        p.place(&end);
    }

    /// Pushes `self_obj`, or 0 in a program with no object.
    pub fn self_obj(&mut self, p: &mut Proc) {
        if self.scope.has_self {
            p.host(hw::SELF_OBJ);
        } else {
            p.push_i(0);
        }
    }

    /// Pushes the object of a placed reference: its slot's exported
    /// variable when the current map is the slot's map, else 0 (E16).
    pub fn slot_object(&mut self, p: &mut Proc, form: &FormKey) {
        let Some(slot) = self.ctx.numbers.slots.get(form) else {
            self.fallback(&format!("reference {form} has no slot"));
            p.push_i(0);
            return;
        };
        let Some(placed) = &slot.placed else {
            self.fallback(&format!("reference {form} is not placed"));
            p.push_i(0);
            return;
        };
        let name = slot.name();
        let map = placed.map_number as i32;
        self.asm.name(&name);
        let other = p.label();
        let end = p.label();
        p.branch_false(&other, |p| {
            p.host(hw::CUR_MAP_INDEX);
            p.push_i(map);
            p.binary("eq");
        });
        p.push_n(&name);
        p.core("fetch_external", 1, 1);
        p.jump(&end);
        p.place(&other);
        p.push_i(0);
        p.place(&end);
    }

    /// Pushes the object a reference expression names.
    pub fn object(&mut self, p: &mut Proc, r: &RefExpr) {
        match r {
            RefExpr::Player => p.host(hw::DUDE_OBJ),
            RefExpr::SelfRef => self.self_obj(p),
            RefExpr::Action => {
                if self.scope.has_self {
                    p.host(hw::SOURCE_OBJ);
                } else {
                    p.push_i(0);
                }
            }
            RefExpr::Placed(form) => self.slot_object(p, form),
            RefExpr::Var(var) => self.deref(p, var),
            RefExpr::Linked(_) => {
                self.fallback("linked references are not in the IR; read as none");
                p.push_i(0);
            }
        }
    }

    /// Pushes the object a call runs on: its `on`, else the running object.
    pub fn subject(&mut self, p: &mut Proc, on: &Option<RefExpr>) {
        match on {
            Some(r) => self.object(p, r),
            None => self.self_obj(p),
        }
    }

    /// Runs `body` with the object of `on` in a temporary when it is not
    /// 0; the temporary's slot is passed to `body`.
    pub fn with_object(
        &mut self,
        p: &mut Proc,
        on: &Option<RefExpr>,
        body: impl FnOnce(&mut Self, &mut Proc, u32),
    ) {
        let t = p.temp();
        self.subject(p, on);
        p.store_local(t);
        let skip = p.label();
        p.branch_false(&skip, |p| p.fetch_local(t));
        body(self, p, t);
        p.at_statement();
        p.place(&skip);
    }

    // ---- expressions -------------------------------------------------------

    /// Pushes the value of `expr`.
    pub fn expr(&mut self, p: &mut Proc, expr: &Expr) -> Ty {
        match expr {
            Expr::Int(v) => {
                p.push_i(*v);
                Ty::Int
            }
            Expr::Float(v) => {
                p.push_f(*v);
                Ty::Float
            }
            Expr::Str(id) => {
                let text = self.text(*id);
                let msg = self.msg(&text);
                self.message_str(p, msg);
                Ty::Str
            }
            Expr::Var(var) => self.load(p, var),
            Expr::Form(form) => {
                match self.ctx.numbers.pids.get(form) {
                    Some(pid) => p.push_i(*pid as i32),
                    None => {
                        self.fallback(&format!("form {form} has no PID; read as 0"));
                        p.push_i(0);
                    }
                }
                Ty::Int
            }
            Expr::Ref(r) => {
                self.object(p, r);
                Ty::Int
            }
            Expr::Call(call) => self.call_value(p, call),
            Expr::Unary(UnOp::Neg, inner) => {
                p.push_i(0);
                let t = self.expr(p, inner);
                p.binary("sub");
                if t == Ty::Float { Ty::Float } else { Ty::Int }
            }
            Expr::Unary(UnOp::Not, _)
            | Expr::Binary(BinOp::And | BinOp::Or | BinOp::Cmp(_), _, _) => {
                self.test_value(p, |g, p, f| g.expr_false(p, expr, f));
                Ty::Int
            }
            Expr::Binary(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), a, b) => {
                let ta = self.expr(p, a);
                let tb = self.expr(p, b);
                p.binary(match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    _ => "mul",
                });
                join(ta, tb)
            }
            Expr::Binary(op @ (BinOp::Div | BinOp::Mod), a, b) => {
                let word = if *op == BinOp::Div { "div" } else { "mod" };
                let ta = self.expr(p, a);
                let literal = matches!(**b, Expr::Int(v) if v != 0)
                    || matches!(**b, Expr::Float(v) if v != 0.0);
                if literal {
                    let tb = self.expr(p, b);
                    p.binary(word);
                    return join(ta, tb);
                }
                let tb = self.expr(p, b);
                let t = p.temp();
                p.store_local(t);
                let zero = p.label();
                let end = p.label();
                p.branch_false(&zero, |p| p.fetch_local(t));
                p.fetch_local(t);
                p.binary(word);
                p.jump(&end);
                p.place(&zero);
                p.pop();
                p.push_i(0);
                p.place(&end);
                join(ta, tb)
            }
        }
    }

    /// Pushes `expr` as an integer (a float is truncated, a string is 0).
    pub fn expr_int(&mut self, p: &mut Proc, expr: &Expr) {
        match self.expr(p, expr) {
            Ty::Int => {}
            Ty::Float => p.unary("ftoi"),
            Ty::Str => {
                p.pop();
                p.push_i(0);
            }
        }
    }

    /// Pushes 1 when the test holds, else 0; `jump_false` emits code that
    /// jumps to its label when the test fails.
    pub fn test_value(
        &mut self,
        p: &mut Proc,
        jump_false: impl FnOnce(&mut Self, &mut Proc, &str),
    ) {
        let fail = p.label();
        let end = p.label();
        jump_false(self, p, &fail);
        p.push_i(1);
        p.jump(&end);
        p.place(&fail);
        p.push_i(0);
        p.place(&end);
    }

    /// Jumps to `label` when `expr` is false (zero).
    pub fn expr_false(&mut self, p: &mut Proc, expr: &Expr, label: &str) {
        match expr {
            Expr::Binary(BinOp::And, a, b) => {
                self.expr_false(p, a, label);
                self.expr_false(p, b, label);
            }
            Expr::Binary(BinOp::Or, a, b) => {
                let ok = p.label();
                self.expr_true(p, a, &ok);
                self.expr_false(p, b, label);
                p.place(&ok);
            }
            Expr::Unary(UnOp::Not, inner) => self.expr_true(p, inner, label),
            Expr::Binary(BinOp::Cmp(op), a, b) => {
                p.branch_false(label, |p| self.compare(p, a, *op, b));
            }
            other => p.branch_false(label, |p| {
                self.expr(p, other);
            }),
        }
    }

    /// Jumps to `label` when `expr` is true (not zero).
    pub fn expr_true(&mut self, p: &mut Proc, expr: &Expr, label: &str) {
        match expr {
            Expr::Binary(BinOp::And, a, b) => {
                let skip = p.label();
                self.expr_false(p, a, &skip);
                self.expr_true(p, b, label);
                p.place(&skip);
            }
            Expr::Binary(BinOp::Or, a, b) => {
                self.expr_true(p, a, label);
                self.expr_true(p, b, label);
            }
            Expr::Unary(UnOp::Not, inner) => self.expr_false(p, inner, label),
            Expr::Binary(BinOp::Cmp(op), a, b) => {
                p.branch_false(label, |p| self.compare(p, a, op.negated(), b));
            }
            other => p.branch_false(label, |p| {
                self.expr(p, other);
                p.unary("not");
            }),
        }
    }

    /// Pushes `a op b` as 0 or 1.
    pub fn compare(&mut self, p: &mut Proc, a: &Expr, op: CmpOp, b: &Expr) {
        self.expr(p, a);
        self.expr(p, b);
        p.binary(cmp_word(op));
    }

    /// Jumps to `label` when `cond` fails.
    pub fn cond_false(&mut self, p: &mut Proc, cond: &Cond, label: &str) {
        match cond {
            Cond::True | Cond::Fallback { .. } => {}
            Cond::False => p.jump(label),
            Cond::Not(inner) => self.cond_true(p, inner, label),
            Cond::All(items) => {
                for item in items {
                    self.cond_false(p, item, label);
                }
            }
            Cond::Any(items) => {
                let ok = p.label();
                for item in items {
                    self.cond_true(p, item, &ok);
                }
                p.jump(label);
                p.place(&ok);
            }
            Cond::Cmp { lhs, op, rhs } => {
                p.branch_false(label, |p| self.compare(p, lhs, *op, rhs));
            }
        }
    }

    /// Jumps to `label` when `cond` holds.
    pub fn cond_true(&mut self, p: &mut Proc, cond: &Cond, label: &str) {
        match cond {
            Cond::True | Cond::Fallback { .. } => p.jump(label),
            Cond::False => {}
            Cond::Not(inner) => self.cond_false(p, inner, label),
            Cond::All(items) => {
                let skip = p.label();
                for item in items {
                    self.cond_false(p, item, &skip);
                }
                p.jump(label);
                p.place(&skip);
            }
            Cond::Any(items) => {
                for item in items {
                    self.cond_true(p, item, label);
                }
            }
            Cond::Cmp { lhs, op, rhs } => {
                p.branch_false(label, |p| self.compare(p, lhs, op.negated(), rhs));
            }
        }
    }

    // ---- statements --------------------------------------------------------

    /// A block body, with `return` going to its end.
    pub fn block(&mut self, p: &mut Proc, body: &[Stmt]) {
        let end = p.label();
        let saved = self.scope.return_label.replace(end.clone());
        self.stmts(p, body);
        p.place(&end);
        self.scope.return_label = saved;
    }

    /// Statements, each leaving the stack as it found it.
    pub fn stmts(&mut self, p: &mut Proc, stmts: &[Stmt]) {
        for stmt in stmts {
            if !p.reachable() {
                self.count(
                    Outcome::Dropped,
                    "unreachable statement after return or a self-stopping word",
                );
                continue;
            }
            let mark = p.mark();
            self.stmt(p, stmt);
            p.at_statement();
            p.release(mark);
        }
    }

    fn stmt(&mut self, p: &mut Proc, stmt: &Stmt) {
        match stmt {
            Stmt::Set(var, value) => self.store(p, var, value),
            Stmt::If { arms, otherwise } => {
                let end = p.label();
                for (cond, body) in arms {
                    let next = p.label();
                    self.expr_false(p, cond, &next);
                    self.stmts(p, body);
                    if p.reachable() {
                        p.jump(&end);
                    }
                    p.place(&next);
                }
                self.stmts(p, otherwise);
                p.place(&end);
            }
            Stmt::Call(call) => self.call_stmt(p, call),
            Stmt::Return => match self.scope.return_label.clone() {
                Some(label) => p.jump(&label),
                None => p.ret(),
            },
        }
    }

    /// Whether a script's blocks are reachable for this program's locals.
    pub fn declare_script_locals(&mut self, script: &SimId, first: u32) -> u32 {
        let mut next = first;
        if let Some(s) = self.ctx.module.scripts.get(script) {
            for var in &s.vars {
                if self.ctx.hoisted.contains_key(&(script.clone(), var.index)) {
                    continue;
                }
                self.local_vars.insert((script.clone(), var.index), next);
                next += 1;
            }
        }
        next
    }

    /// The group whose parent is `form`.
    pub fn group_of(&self, form: &FormKey) -> Option<SimId> {
        self.ctx.group_of_parent.get(form).cloned()
    }
}

/// The comparison word.
pub(crate) fn cmp_word(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "eq",
        CmpOp::Ne => "ne",
        CmpOp::Lt => "lt",
        CmpOp::Le => "le",
        CmpOp::Gt => "gt",
        CmpOp::Ge => "ge",
    }
}

fn join(a: Ty, b: Ty) -> Ty {
    match (a, b) {
        (Ty::Str, _) | (_, Ty::Str) => Ty::Str,
        (Ty::Float, _) | (_, Ty::Float) => Ty::Float,
        _ => Ty::Int,
    }
}

/// A variable for the report.
pub(crate) fn var_name(var: &VarRef) -> String {
    match var {
        VarRef::Local { script, index, .. } => format!("{script} var {index}"),
        VarRef::Quest { quest, index, .. } => format!("{quest} var {index}"),
        VarRef::Global(id) => id.to_string(),
        VarRef::Hoisted { owner, index, .. } => format!("{owner}.var {index}"),
        VarRef::Temp(n) => format!("temp {n}"),
    }
}
