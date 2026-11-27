//! Assembler text with a stack discipline that makes the SPEC-017 section 8
//! stack model find nothing, by construction.
//!
//! **The discipline.** [`Proc`] counts the value-stack depth of the code it
//! emits, above the procedure's frame:
//!
//! - every expression pushes exactly one value; every statement leaves the
//!   depth where it found it (a host word's unused result is popped at
//!   once);
//! - every jump records the depth at its target, and a label placed on a
//!   reachable path must meet the same depth, so paths only ever join at
//!   equal depths (the model's merge rule never sees a conflict);
//! - jump targets are always direct `push.a` constants, so the model
//!   resolves every jump and branch;
//! - procedure temporaries are frame locals pushed right after `frame`, so
//!   the depth at statement level is the temporaries' count and nothing
//!   else;
//! - every procedure ends in the one epilogue: a result (or the placeholder
//!   0 a host call needs above its saved context, SPEC-004 section 3.6),
//!   `v_to_r drop_frame r_to_v restore_base ret`; a `return` jumps there at
//!   statement depth;
//! - local calls use the SPEC-004 section 3.6 protocol with zero arguments
//!   (`push.a back v_to_r push.i 0 push.p name call`), which the model
//!   reads as one pushed result.
//!
//! A break of the discipline is a bug of the generator and panics in
//! debug builds; the tests run every generated program through the engine's
//! own model as well.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::words::HostWord;

/// One procedure's code.
#[derive(Debug, Clone)]
pub(crate) struct Proc {
    pub name: String,
    code: Vec<String>,
    /// Depth above the temporaries.
    depth: u32,
    reachable: bool,
    /// Label to the depth its jumps arrive with.
    targets: BTreeMap<String, u32>,
    next_label: u32,
    /// Initial values of the frame temporaries (the high-water mark).
    temps: Vec<i32>,
    /// The next temporary [`Proc::temp`] hands out; below it, in use.
    next_temp: u32,
    /// Temporaries below this are never released (a result, `Temp`
    /// variables).
    floor: u32,
    /// The temporary that holds the result, for a value procedure.
    result: Option<u32>,
    /// The deepest the expression stack went above the temporaries.
    max_depth: u32,
    /// Per value above the temporaries, its constant when the code that
    /// pushed it was a `push.i` (else `None`): what the local words read
    /// their index from.
    known: Vec<Option<i32>>,
    /// Per label, the constants every jump to it arrived with (a value
    /// two paths disagree on is `None`).
    known_at_label: BTreeMap<String, Vec<Option<i32>>>,
    /// Labels already placed: a jump arriving later is a backward one.
    placed: std::collections::BTreeSet<String>,
    /// The instance locals the procedure reads or writes.
    locals: LocalUse,
}

/// The instance locals (`local_var`, `set_local_var`) code uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LocalUse {
    /// The constant indices.
    pub indices: std::collections::BTreeSet<i32>,
    /// Whether some index is not a `push.i` constant (it cannot be checked
    /// against the program's slice).
    pub dynamic: bool,
}

impl LocalUse {
    /// The slice the code needs: one past the highest index.
    #[cfg(test)]
    pub fn needed(&self) -> u32 {
        self.indices
            .last()
            .map_or(0, |&i| u32::try_from(i).map_or(u32::MAX, |i| i + 1))
    }

    /// Whether every index is a constant inside a slice of `count`.
    pub fn fits(&self, count: u32) -> bool {
        !self.dynamic && self.indices.iter().all(|&i| i >= 0 && (i as u32) < count)
    }

    #[cfg(test)]
    fn add(&mut self, other: &LocalUse) {
        self.indices.extend(other.indices.iter().copied());
        self.dynamic |= other.dynamic;
    }
}

/// The most values one procedure may hold above its frame base (its
/// temporaries plus its deepest expression). The value stack holds 341
/// values (SPEC-004 section 3.1, the stack model's limit); the rest is
/// left for the program variables, the saved host-call context and the
/// frames of nested local calls.
pub const MAX_FRAME_VALUES: usize = 160;

impl Proc {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            code: Vec::new(),
            depth: 0,
            reachable: true,
            targets: BTreeMap::new(),
            next_label: 0,
            temps: Vec::new(),
            next_temp: 0,
            floor: 0,
            result: None,
            max_depth: 0,
            known: Vec::new(),
            known_at_label: BTreeMap::new(),
            placed: std::collections::BTreeSet::new(),
            locals: LocalUse::default(),
        }
    }

    /// A procedure whose result is its first temporary (initially `0`).
    pub fn with_result(name: &str) -> Self {
        let mut proc = Self::new(name);
        let slot = proc.temp();
        proc.result = Some(slot);
        proc.keep(slot);
        proc
    }

    /// The temporary holding the result.
    pub fn result_slot(&self) -> Option<u32> {
        self.result
    }

    /// A frame temporary, 0 when handed out. A slot freed by
    /// [`Proc::release`] is reused (and zeroed here), so a procedure holds
    /// only as many temporaries as one statement needs at once: they sit on
    /// the value stack above the frame, and the stack holds 341 values
    /// (SPEC-004 section 3.1, the stack model's `MAX_DEPTH`).
    pub fn temp(&mut self) -> u32 {
        let slot = self.next_temp;
        self.next_temp += 1;
        if (slot as usize) < self.temps.len() {
            self.push_i(0);
            self.store_local(slot);
        } else {
            self.temps.push(0);
        }
        slot
    }

    /// The allocation point, for [`Proc::release`].
    pub fn mark(&self) -> u32 {
        self.next_temp
    }

    /// Frees the temporaries handed out since `mark` (not below the kept
    /// ones): the code that used them is complete.
    pub fn release(&mut self, mark: u32) {
        self.next_temp = mark.max(self.floor).min(self.next_temp.max(self.floor));
    }

    /// Never releases `slot` (or anything below it).
    pub fn keep(&mut self, slot: u32) {
        self.floor = self.floor.max(slot + 1);
    }

    /// The instance locals the procedure uses.
    pub fn locals(&self) -> &LocalUse {
        &self.locals
    }

    /// The most values the procedure holds above its frame base.
    pub fn frame_values(&self) -> usize {
        self.temps.len() + self.max_depth as usize
    }

    /// Whether code emitted now can run.
    pub fn reachable(&self) -> bool {
        self.reachable
    }

    /// A fresh label of this procedure.
    pub fn label(&mut self) -> String {
        self.next_label += 1;
        format!("{}.{}", self.name, self.next_label)
    }

    /// The epilogue's label.
    pub fn end_label(&self) -> String {
        format!("{}.end", self.name)
    }

    fn line(&mut self, text: impl Into<String>) {
        self.code.push(format!("    {}", text.into()));
    }

    fn change(&mut self, pops: u32, pushes: u32) {
        if self.reachable {
            debug_assert!(
                self.depth >= pops,
                "{}: stack underflow in generated code ({} < {pops}) at {:?}",
                self.name,
                self.depth,
                self.code.last()
            );
            self.depth = self.depth.saturating_sub(pops) + pushes;
            self.max_depth = self.max_depth.max(self.depth);
            let keep = self.known.len().saturating_sub(pops as usize);
            self.known.truncate(keep);
            self.known.extend((0..pushes).map(|_| None));
        }
    }

    /// The constant `below` values under the top, when it is one.
    fn known_at(&self, below: usize) -> Option<i32> {
        self.known
            .len()
            .checked_sub(below + 1)
            .and_then(|at| self.known[at])
    }

    pub fn push_i(&mut self, value: i32) {
        self.line(format!("push.i {value}"));
        self.change(0, 1);
        if self.reachable
            && let Some(top) = self.known.last_mut()
        {
            *top = Some(value);
        }
    }

    pub fn push_f(&mut self, value: f32) {
        let value = if value.is_finite() { value } else { 0.0 };
        let mut text = format!("{value:?}");
        if !text.contains(['.', 'e', 'E']) {
            text.push_str(".0");
        }
        self.line(format!("push.f {text}"));
        self.change(0, 1);
    }

    pub fn push_a(&mut self, label: &str) {
        self.line(format!("push.a {label}"));
        self.change(0, 1);
    }

    pub fn push_p(&mut self, procedure: &str) {
        self.line(format!("push.p {procedure}"));
        self.change(0, 1);
    }

    pub fn push_n(&mut self, name: &str) {
        self.line(format!("push.n {name}"));
        self.change(0, 1);
    }

    /// A core word with its stack effect.
    pub fn core(&mut self, mnemonic: &str, pops: u32, pushes: u32) {
        self.line(mnemonic);
        self.change(pops, pushes);
    }

    /// A host word; its operands must be on the stack.
    pub fn host(&mut self, word: HostWord) {
        // The index is the first operand: under the value for a write.
        let index = match word.name {
            "local_var" => Some(0),
            "set_local_var" => Some(1),
            _ => None,
        };
        if let Some(below) = index
            && self.reachable
        {
            match self.known_at(below) {
                Some(i) => {
                    self.locals.indices.insert(i);
                }
                None => self.locals.dynamic = true,
            }
        }
        self.line(word.name);
        self.change(u32::from(word.args), u32::from(word.push));
    }

    /// A host word as a statement: an unused result is popped.
    pub fn host_stmt(&mut self, word: HostWord) {
        self.host(word);
        if word.push {
            self.pop();
        }
    }

    pub fn pop(&mut self) {
        self.core("pop", 1, 0);
    }

    /// `a, b -> a op b` for a binary core word.
    pub fn binary(&mut self, mnemonic: &str) {
        self.core(mnemonic, 2, 1);
    }

    /// `a -> op a` for a unary core word.
    pub fn unary(&mut self, mnemonic: &str) {
        self.core(mnemonic, 1, 1);
    }

    fn arrive(&mut self, label: &str) {
        if !self.reachable {
            return;
        }
        let known = self.known.clone();
        if self.placed.contains(label) {
            // A backward jump: the code after the label was read with the
            // constants recorded there; if this path disagrees, no index
            // read under them can be trusted.
            if self.known_at_label.get(label) != Some(&known) {
                self.locals.dynamic = true;
            }
        } else {
            self.known_at_label
                .entry(label.to_owned())
                .and_modify(|seen| *seen = merge_known(seen, &known))
                .or_insert(known);
        }
        let depth = self.depth;
        let recorded = *self.targets.entry(label.to_owned()).or_insert(depth);
        debug_assert_eq!(
            recorded, depth,
            "{}: paths meet at {label} with depths {recorded} and {depth}",
            self.name
        );
    }

    /// An unconditional jump.
    pub fn jump(&mut self, label: &str) {
        self.push_a(label);
        self.core("jmp", 1, 0);
        self.arrive(label);
        self.reachable = false;
    }

    /// `bz` to `label`: the caller pushed `push.a label` and then the
    /// condition (see [`Proc::branch_false`]).
    fn bz(&mut self, label: &str) {
        self.core("bz", 2, 0);
        self.arrive(label);
    }

    /// Jump to `label` when the value `condition` pushes is zero.
    pub fn branch_false(&mut self, label: &str, condition: impl FnOnce(&mut Self)) {
        self.push_a(label);
        let before = self.depth;
        condition(self);
        debug_assert!(
            !self.reachable || self.depth == before + 1,
            "{}: a condition pushed {} values",
            self.name,
            self.depth as i64 - before as i64
        );
        self.bz(label);
    }

    /// Places a label.
    pub fn place(&mut self, label: &str) {
        match self.targets.get(label) {
            Some(&recorded) => {
                if self.reachable {
                    debug_assert_eq!(
                        recorded, self.depth,
                        "{}: fall-through meets {label} at depth {} (recorded {recorded})",
                        self.name, self.depth
                    );
                }
                // Paths join here: a constant all of them carry stays known.
                let arrived = self
                    .known_at_label
                    .get(label)
                    .cloned()
                    .unwrap_or_else(|| vec![None; recorded as usize]);
                self.known = if self.reachable {
                    merge_known(&arrived, &self.known)
                } else {
                    arrived
                };
                self.depth = recorded;
                self.reachable = true;
            }
            None => {
                if self.reachable {
                    self.targets.insert(label.to_owned(), self.depth);
                    self.known_at_label
                        .insert(label.to_owned(), self.known.clone());
                }
            }
        }
        self.placed.insert(label.to_owned());
        self.code.push(format!("{label}:"));
    }

    /// A local call of a procedure of this program (no arguments); pushes
    /// its result.
    pub fn call(&mut self, procedure: &str) {
        let back = self.label();
        self.push_a(&back);
        self.core("v_to_r", 1, 0);
        self.push_i(0);
        self.push_p(procedure);
        // The model reads `call` as popping the count and index, then the
        // counted arguments, and pushing one result.
        self.core("call", 2, 1);
        self.code.push(format!("{back}:"));
    }

    /// A local call as a statement.
    pub fn call_stmt(&mut self, procedure: &str) {
        self.call(procedure);
        self.pop();
    }

    pub fn fetch_local(&mut self, slot: u32) {
        self.push_i(slot as i32);
        self.core("fetch_local", 1, 1);
    }

    /// Stores the value on top into a temporary.
    pub fn store_local(&mut self, slot: u32) {
        self.push_i(slot as i32);
        self.core("store_local", 2, 0);
    }

    pub fn fetch_global(&mut self, index: u32) {
        self.push_i(index as i32);
        self.core("fetch_global", 1, 1);
    }

    /// Stores the value on top into a program variable.
    pub fn store_global(&mut self, index: u32) {
        self.push_i(index as i32);
        self.core("store_global", 2, 0);
    }

    /// `return`: to the epilogue, at statement depth.
    pub fn ret(&mut self) {
        debug_assert!(
            !self.reachable || self.depth == 0,
            "{}: return at depth {}",
            self.name,
            self.depth
        );
        let end = self.end_label();
        self.jump(&end);
    }

    /// Checks the statement-level invariant.
    pub fn at_statement(&self) {
        debug_assert!(
            !self.reachable || self.depth == 0,
            "{}: a statement left {} values",
            self.name,
            self.depth
        );
    }

    /// The procedure's text: label, frame, temporaries, body, epilogue.
    pub fn finish(mut self) -> Vec<String> {
        self.at_statement();
        let end = self.end_label();
        self.place(&end);
        match self.result {
            Some(slot) => {
                self.fetch_local(slot);
            }
            None => self.push_i(0),
        }
        for word in ["v_to_r", "drop_frame", "r_to_v", "restore_base", "ret"] {
            self.line(word);
        }
        let mut out = vec![format!("{}:", self.name), "    frame".to_owned()];
        for value in &self.temps {
            out.push(format!("    push.i {value}"));
        }
        out.extend(self.code);
        out
    }
}

/// The constants two paths agree on, value by value.
fn merge_known(a: &[Option<i32>], b: &[Option<i32>]) -> Vec<Option<i32>> {
    debug_assert_eq!(a.len(), b.len(), "paths meet at different depths");
    a.iter()
        .zip(b)
        .map(|(x, y)| if x == y { *x } else { None })
        .collect()
}

/// One program's assembler source.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProgramAsm {
    /// Procedures in record order (`start` is record 1; record 0 shares
    /// its body).
    procs: Vec<(String, Vec<String>)>,
    /// Program variables (the global area): name and initial value.
    pvars: Vec<(String, i32)>,
    /// External variable names (`.name`).
    names: Vec<String>,
    /// Names exported in the initial stream.
    exports: Vec<String>,
    /// The largest [`Proc::frame_values`] of the procedures, and whose.
    deepest: (usize, String),
    /// Each procedure's instance locals, by name.
    proc_locals: Vec<(String, LocalUse)>,
}

impl ProgramAsm {
    /// A program variable's index, added on first use.
    pub fn pvar(&mut self, name: &str, initial: i32) -> u32 {
        if let Some(index) = self.pvars.iter().position(|(n, _)| n == name) {
            return index as u32;
        }
        self.pvars.push((name.to_owned(), initial));
        (self.pvars.len() - 1) as u32
    }

    /// Declares an external variable name.
    pub fn name(&mut self, name: &str) {
        if !self.names.iter().any(|n| n == name) {
            self.names.push(name.to_owned());
        }
    }

    /// Exports `name` in the initial stream.
    pub fn export(&mut self, name: &str) {
        self.name(name);
        if !self.exports.iter().any(|n| n == name) {
            self.exports.push(name.to_owned());
        }
    }

    /// Whether a procedure is declared.
    pub fn has(&self, name: &str) -> bool {
        self.procs.iter().any(|(n, _)| n == name)
    }

    /// Adds a finished procedure (replacing none: names are unique).
    pub fn add(&mut self, proc: Proc) {
        debug_assert!(!self.has(&proc.name), "procedure {} twice", proc.name);
        let name = proc.name.clone();
        if proc.frame_values() > self.deepest.0 {
            self.deepest = (proc.frame_values(), name.clone());
        }
        self.proc_locals.push((name.clone(), proc.locals().clone()));
        self.procs.push((name, proc.finish()));
    }

    /// The largest frame of the procedures, and whose.
    pub fn deepest(&self) -> &(usize, String) {
        &self.deepest
    }

    /// The instance locals of all procedures.
    #[cfg(test)]
    pub fn locals(&self) -> LocalUse {
        let mut all = LocalUse::default();
        for (_, used) in &self.proc_locals {
            all.add(used);
        }
        all
    }

    /// The first procedure whose locals do not fit a slice of `count`,
    /// with what it uses.
    pub fn local_offender(&self, count: u32) -> Option<&(String, LocalUse)> {
        self.proc_locals.iter().find(|(_, u)| !u.fits(count))
    }

    /// How many program variables (values of the global area) it has.
    pub fn pvar_count(&self) -> usize {
        self.pvars.len()
    }

    /// The procedure names in record order, `root` first.
    pub fn procedure_names(&self) -> Vec<String> {
        let mut out = vec!["root".to_owned()];
        out.extend(self.procs.iter().map(|(n, _)| n.clone()));
        out
    }

    /// The source text. `start` must have been added first.
    pub fn text(&self, title: &str) -> String {
        debug_assert_eq!(self.procs.first().map(|(n, _)| n.as_str()), Some("start"));
        let mut out = String::new();
        let _ = writeln!(out, "; {title}");
        let _ = writeln!(out, ".procedure root body=start");
        for (name, _) in &self.procs {
            let _ = writeln!(out, ".procedure {name}");
        }
        for name in &self.names {
            let _ = writeln!(out, ".name {name}");
        }
        let _ = writeln!(out, ".entry init");
        let _ = writeln!(out, "init:");
        let _ = writeln!(out, "    global_area");
        for (name, value) in &self.pvars {
            let _ = writeln!(out, "    push.i {value} ; {name}");
        }
        for name in &self.exports {
            let _ = writeln!(out, "    push.n {name}");
            let _ = writeln!(out, "    export_var");
        }
        let _ = writeln!(out, "    push.i 0");
        let _ = writeln!(out, "    critical_off");
        let _ = writeln!(out, "    push.a start");
        let _ = writeln!(out, "    jmp");
        for (_, lines) in &self.procs {
            for line in lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}
