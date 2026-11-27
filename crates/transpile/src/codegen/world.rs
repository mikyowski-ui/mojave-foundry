//! Enable groups (spec 8.3), locks (9.3), terminals (9.1) and notes (9.2).

use std::collections::BTreeMap;

use super::asm::Proc;
use super::generator::{Gen, Section, locals};
use super::words::{self as hw};
use crate::ir::{FormKey, PlacedKind, RefExpr, SimId, Terminal};
use crate::lowered::Outcome;

/// One child of a group as the reference table of spec 8.3 holds it.
#[derive(Debug, Clone)]
struct TableRow {
    map: u32,
    hex: u32,
    elevation: u8,
    pid: u32,
    opposite: bool,
}

/// A lock's required Lockpick level and its experience index (spec 9.3).
pub(crate) fn lock_requirement(level: u8) -> (i32, usize) {
    match level {
        0 => (0, 0),
        1..=25 => (25, 1),
        26..=50 => (50, 2),
        51..=75 => (75, 3),
        _ => (100, 4),
    }
}

impl Gen<'_, '_> {
    /// The group applier procedure's name, generated with the program.
    pub fn group_proc(&mut self, group: &SimId) -> String {
        let index = self
            .ctx
            .module
            .groups
            .keys()
            .position(|g| g == group)
            .unwrap_or(0);
        let name = format!("grp_{}", index + 1);
        self.pending_procs.insert(name.clone());
        name
    }

    fn group_rows(&mut self, group: &SimId) -> Vec<TableRow> {
        let module = self.ctx.module;
        let mut members: Vec<(FormKey, bool)> = module
            .state
            .values()
            .flatten()
            .filter(|row| &row.group == group && !row.mobile)
            .map(|row| (row.reference.clone(), row.opposite))
            .collect();
        if members.is_empty() {
            let Some(g) = module.groups.get(group) else {
                return Vec::new();
            };
            members = g.children.clone();
            if !members.iter().any(|(c, _)| c == &group.form) {
                members.push((group.form.clone(), false));
            }
        }
        let mut rows = Vec::new();
        for (child, opposite) in members {
            let Some(object) = self.ctx.facts.placement.of_reference(&child).next() else {
                continue;
            };
            if object.kind == PlacedKind::Critter {
                // Critters apply their own state (spec 8.3).
                continue;
            }
            if let Some(pid) = object.pid {
                rows.push(TableRow {
                    map: object.map_number,
                    hex: object.hex,
                    elevation: object.elevation,
                    pid,
                    opposite,
                });
            }
        }
        rows
    }

    /// The reference table of a group (spec 8.3): every child on the
    /// current map is shown or hidden by the group's state; `only` keeps
    /// the rows of one map.
    pub fn apply_group(&mut self, p: &mut Proc, group: &SimId, only: Option<u32>) {
        let rows = self.group_rows(group);
        if rows.iter().all(|r| only.is_some_and(|m| m != r.map)) {
            return;
        }
        let Some(g) = self.global(group) else {
            return;
        };
        let mut by_map: BTreeMap<u32, Vec<TableRow>> = BTreeMap::new();
        for row in rows {
            if only.is_none_or(|m| m == row.map) {
                by_map.entry(row.map).or_default().push(row);
            }
        }
        for (map, rows) in by_map {
            let skip = p.label();
            p.branch_false(&skip, |p| {
                p.host(hw::CUR_MAP_INDEX);
                p.push_i(map as i32);
                p.binary("eq");
            });
            for row in rows {
                p.push_i(row.hex as i32);
                p.push_i(i32::from(row.elevation));
                p.push_i(row.pid as i32);
                p.host(hw::TILE_CONTAINS_PID_OBJ);
                self.get_global(p, g);
                p.push_i(0);
                p.binary(if row.opposite { "ne" } else { "eq" });
                p.host(hw::SET_OBJ_VISIBILITY);
                self.count_in(Section::State, Outcome::Compiled, "");
            }
            p.place(&skip);
        }
    }

    /// `Enable` / `Disable` (spec 8.3).
    pub fn enable(&mut self, p: &mut Proc, on: &Option<RefExpr>, enable: bool) {
        if let Some(RefExpr::Placed(form)) = on
            && let Some(group) = self.group_of(form)
        {
            if let Some(g) = self.global(&group) {
                self.set_global(p, g, i32::from(enable));
                let proc = self.group_proc(&group);
                p.call_stmt(&proc);
            }
            self.count_in(Section::State, Outcome::Compiled, "");
            return;
        }
        if let Some(RefExpr::Placed(form)) = on
            && self
                .ctx
                .numbers
                .slots
                .get(form)
                .and_then(|s| s.placed.as_ref())
                .is_none()
        {
            // Placed on a map the slice does not build: nothing to show
            // or hide here (and no group: a group needs a child in the
            // slice).
            self.count_in(
                Section::State,
                Outcome::Dropped,
                "Enable/Disable of a reference outside the slice: no effect",
            );
            return;
        }
        self.with_object(p, on, |_, p, t| {
            p.fetch_local(t);
            p.push_i(i32::from(!enable));
            p.host(hw::SET_OBJ_VISIBILITY);
        });
        self.count_in(
            Section::State,
            Outcome::Fallback,
            "Enable/Disable of a reference in no group: applied on the current map only",
        );
    }

    /// In a critter program: hide or show this instance by the groups its
    /// placements are children of (spec 8.3, critter children).
    pub fn apply_self_groups(&mut self, p: &mut Proc) {
        let Some(base) = self.actor.clone() else {
            return;
        };
        let entries: Vec<(u32, SimId, bool)> = self
            .ctx
            .numbers
            .slots
            .iter()
            .filter(|(_, s)| s.placed.as_ref().and_then(|o| o.base.as_ref()) == Some(&base))
            .flat_map(|(form, s)| {
                self.ctx
                    .groups_of_child
                    .get(form)
                    .into_iter()
                    .flatten()
                    .map(move |(g, o)| (s.k, g.clone(), *o))
            })
            .collect();
        for (k, group, opposite) in entries {
            let Some(g) = self.global(&group) else {
                continue;
            };
            let skip = p.label();
            p.branch_false(&skip, |p| {
                p.push_i(locals::SLOT as i32);
                p.host(hw::LOCAL_VAR);
                p.push_i(k as i32);
                p.binary("eq");
            });
            p.host(hw::SELF_OBJ);
            self.get_global(p, g);
            p.push_i(0);
            p.binary(if opposite { "ne" } else { "eq" });
            p.host(hw::SET_OBJ_VISIBILITY);
            p.place(&skip);
            self.count_in(Section::State, Outcome::Compiled, "");
        }
    }

    // ---- locks -------------------------------------------------------------

    /// `use_p_proc` part of a lock: the key opens it (spec 9.3).
    pub fn lock_use(&mut self, p: &mut Proc, key: &Option<FormKey>) {
        let Some(key) = key else {
            return;
        };
        let Some(&pid) = self.ctx.numbers.pids.get(key) else {
            self.count_in(
                Section::Terminals,
                Outcome::Dropped,
                "lock key with no PID: the lock is left to the skill",
            );
            return;
        };
        let skip = p.label();
        p.branch_false(&skip, |p| {
            p.host(hw::SELF_OBJ);
            p.host(hw::OBJ_IS_LOCKED);
        });
        p.branch_false(&skip, |p| {
            p.host(hw::DUDE_OBJ);
            p.push_i(pid as i32);
            p.host(hw::OBJ_IS_CARRYING_OBJ_PID);
        });
        p.host(hw::SELF_OBJ);
        p.host(hw::OBJ_UNLOCK);
        let label = self.label(15);
        self.message_str(p, label);
        p.host(hw::DISPLAY_MSG);
        p.place(&skip);
        self.count_in(Section::Terminals, Outcome::Compiled, "");
    }

    /// `use_skill_on_p_proc` part of a lock: Lockpick at the level.
    pub fn lock_skill(&mut self, p: &mut Proc, level: u8) {
        let skip = p.label();
        p.branch_false(&skip, |p| {
            p.host(hw::ACTION_BEING_USED);
            p.push_i(9);
            p.binary("eq");
        });
        p.branch_false(&skip, |p| {
            p.host(hw::SELF_OBJ);
            p.host(hw::OBJ_IS_LOCKED);
        });
        let hard = p.label();
        let done = p.label();
        if level == 255 {
            p.jump(&hard);
        } else {
            let (required, xp) = lock_requirement(level);
            let xp = self.ctx.settings.lockpick_xp[xp];
            p.branch_false(&hard, |p| {
                p.host(hw::DUDE_OBJ);
                p.push_i(9);
                p.host(hw::HAS_SKILL);
                p.push_i(required);
                p.binary("ge");
            });
            p.host(hw::SELF_OBJ);
            p.host(hw::OBJ_UNLOCK);
            if xp > 0 {
                p.push_i(xp);
                p.host(hw::GIVE_EXP_POINTS);
            }
            p.jump(&done);
        }
        p.place(&hard);
        let label = self.label(16);
        self.message_str(p, label);
        p.host(hw::DISPLAY_MSG);
        p.place(&done);
        p.host(hw::SCRIPT_OVERRIDES);
        p.place(&skip);
        self.count_in(Section::Terminals, Outcome::Compiled, "");
    }

    // ---- terminals ---------------------------------------------------------

    /// The menus of a terminal program, each with the first menu that
    /// opens it.
    fn menu_nodes(&self, menus: &[SimId]) -> Vec<(SimId, Option<usize>)> {
        menus
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let parent = (i > 0)
                    .then(|| {
                        menus.iter().position(|m| {
                            self.ctx.module.terminals.get(m).is_some_and(|t| {
                                t.items.iter().any(|item| item.submenu.as_ref() == Some(id))
                            })
                        })
                    })
                    .flatten();
                (id.clone(), parent)
            })
            .collect()
    }

    /// The terminal and its sub-terminals, breadth first, with each node's
    /// parent.
    fn terminal_nodes(&self, root: &SimId) -> Vec<(SimId, Option<usize>)> {
        let mut nodes: Vec<(SimId, Option<usize>)> = vec![(root.clone(), None)];
        let mut at = 0;
        while at < nodes.len() {
            let id = nodes[at].0.clone();
            if let Some(t) = self.ctx.module.terminals.get(&id) {
                for item in &t.items {
                    if let Some(sub) = &item.submenu
                        && self.ctx.module.terminals.contains_key(sub)
                        && !nodes.iter().any(|(n, _)| n == sub)
                    {
                        nodes.push((sub.clone(), Some(at)));
                    }
                }
            }
            at += 1;
        }
        nodes
    }

    /// The procedures of a terminal program (spec 9.1): `use_p_proc` and
    /// `use_skill_on_p_proc` open a no-head dialogue whose nodes are the
    /// terminal and its sub-terminals.
    pub fn terminal(&mut self, root: &SimId) {
        let program = self.ctx.module.terminal_programs.get(root).cloned();
        let nodes = match &program {
            Some(program) => self.menu_nodes(&program.menus),
            None => self.terminal_nodes(root),
        };
        let hack = match &program {
            Some(program) => program.hack.clone(),
            None => self.ctx.module.terminals.get(root).and_then(|t| {
                (t.flags & crate::terminals::UNLOCKED == 0).then(|| crate::ir::Hack {
                    science: crate::terminals::hack_skill(t.difficulty),
                    xp: self.ctx.settings.hack_xp[usize::from(t.difficulty.min(4))],
                    password: t.password.clone(),
                })
            }),
        };
        let Some(top) = self.ctx.module.terminals.get(root).cloned() else {
            return;
        };
        self.scope.section = Some(Section::Terminals);
        let reply_var = self.asm.pvar("t_reply", 0);

        // Opening.
        for (proc_name, skill) in [("use_p_proc", false), ("use_skill_on_p_proc", true)] {
            let mut p = Proc::new(proc_name);
            let skip = p.label();
            if skill {
                p.branch_false(&skip, |p| {
                    p.host(hw::ACTION_BEING_USED);
                    p.push_i(12);
                    p.binary("eq");
                });
            }
            p.push_i(self.list);
            p.host(hw::SELF_OBJ);
            p.push_i(4);
            p.push_i(-1);
            p.push_i(-1);
            p.host(hw::START_GDIALOG);
            p.host(hw::GSAY_START);
            p.push_i(0);
            p.store_global(reply_var);
            p.call_stmt("n_1");
            p.host(hw::GSAY_END);
            p.host(hw::END_DIALOGUE);
            p.host(hw::SCRIPT_OVERRIDES);
            p.place(&skip);
            self.asm.add(p);
        }

        let _ = &top;
        for (index, (id, parent)) in nodes.iter().enumerate() {
            let Some(terminal) = self.ctx.module.terminals.get(id).cloned() else {
                continue;
            };
            let lock = if index == 0 { hack.as_ref() } else { None };
            self.terminal_node(index, &terminal, *parent, lock, &nodes, reply_var);
        }
        if let Some(hack) = &hack {
            self.terminal_unlock(hack.xp);
        }
        self.count_in(Section::Terminals, Outcome::Compiled, "");
    }

    fn terminal_node(
        &mut self,
        index: usize,
        terminal: &Terminal,
        parent: Option<usize>,
        lock: Option<&crate::ir::Hack>,
        nodes: &[(SimId, Option<usize>)],
        reply_var: u32,
    ) {
        let name = format!("n_{}", index + 1);
        let mut p = Proc::new(&name);
        let header = match terminal.header {
            Some(text) => {
                let text = self.text(text);
                self.msg(&text)
            }
            None => self.label(1),
        };
        // Reply: a pending result text, else the header.
        let plain = p.label();
        let replied = p.label();
        p.branch_false(&plain, |p| p.fetch_global(reply_var));
        p.push_i(self.list);
        p.fetch_global(reply_var);
        p.host(hw::GSAY_REPLY);
        p.jump(&replied);
        p.place(&plain);
        p.push_i(self.list);
        p.push_i(header);
        p.host(hw::GSAY_REPLY);
        p.place(&replied);
        p.push_i(0);
        p.store_global(reply_var);

        let menu = p.label();
        if let Some(hack) = lock {
            p.branch_false(&menu, |p| {
                p.push_i(locals::UNLOCKED as i32);
                p.host(hw::LOCAL_VAR);
                p.push_i(0);
                p.binary("eq");
            });
            if let Some(required) = hack.science {
                let required = i32::from(required);
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.host(hw::DUDE_OBJ);
                    p.push_i(12);
                    p.host(hw::HAS_SKILL);
                    p.push_i(required);
                    p.binary("ge");
                });
                let open = self.label(12);
                let close = self.label(13);
                p.push_i(self.list);
                self.message_str(&mut p, open);
                p.push_i(required);
                p.binary("add");
                self.message_str(&mut p, close);
                p.binary("add");
                p.push_p("h_hack");
                p.push_i(50);
                p.host(hw::GSAY_OPTION);
                p.place(&skip);
            }
            if let Some(note) = &hack.password
                && let Some(g) = self.ctx.numbers.global(note)
            {
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.push_i(g as i32);
                    p.host(hw::GLOBAL_VAR);
                });
                let label = self.label(11);
                p.push_i(self.list);
                p.push_i(label);
                p.push_p("h_password");
                p.push_i(50);
                p.host(hw::GSAY_OPTION);
                p.place(&skip);
            }
            let exit = self.label(5);
            p.push_i(self.list);
            p.push_i(exit);
            p.push_i(0);
            p.push_i(50);
            p.host(hw::GSAY_OPTION);
            p.ret();
        }
        p.place(&menu);
        for (i, item) in terminal.items.iter().enumerate() {
            let mark = p.mark();
            let skip = p.label();
            self.cond_false(&mut p, &item.condition, &skip);
            let text = self.text(item.text);
            let id = self.msg(&text);
            p.push_i(self.list);
            p.push_i(id);
            p.push_p(&format!("o_{}_{}", index + 1, i + 1));
            p.push_i(50);
            p.host(hw::GSAY_OPTION);
            p.place(&skip);
            p.release(mark);
            self.terminal_item(index, i, item, nodes, reply_var);
        }
        match parent {
            Some(parent) => {
                let back = self.label(4);
                p.push_i(self.list);
                p.push_i(back);
                p.push_p(&format!("n_{}", parent + 1));
                p.push_i(50);
                p.host(hw::GSAY_OPTION);
            }
            None => {
                let exit = self.label(5);
                p.push_i(self.list);
                p.push_i(exit);
                p.push_i(0);
                p.push_i(50);
                p.host(hw::GSAY_OPTION);
            }
        }
        self.asm.add(p);
    }

    fn terminal_item(
        &mut self,
        node: usize,
        index: usize,
        item: &crate::ir::MenuItem,
        nodes: &[(SimId, Option<usize>)],
        reply_var: u32,
    ) {
        let mut p = Proc::new(&format!("o_{}_{}", node + 1, index + 1));
        self.scope.temps.clear();
        if let Some(script) = &item.script {
            let body = self
                .ctx
                .module
                .scripts
                .get(script)
                .and_then(|s| s.blocks.first())
                .map(|b| b.body.clone());
            if let Some(body) = body {
                self.block(&mut p, &body);
            }
        }
        if let Some(note) = &item.show_note
            && item.add_note
        {
            self.add_note(&mut p, note);
        }
        let mut reply = item.result.map(|t| self.text(t));
        if reply.is_none()
            && let Some(note) = &item.show_note
        {
            reply = self.note_text(note);
        }
        if let Some(text) = reply {
            let id = self.msg(&text);
            p.push_i(id);
            p.store_global(reply_var);
        }
        let next = item
            .submenu
            .as_ref()
            .and_then(|sub| nodes.iter().position(|(n, _)| n == sub))
            .unwrap_or(node);
        p.call_stmt(&format!("n_{}", next + 1));
        self.asm.add(p);
    }

    fn terminal_unlock(&mut self, xp: i32) {
        for name in ["h_hack", "h_password"] {
            let mut p = Proc::new(name);
            self.set_local(&mut p, locals::UNLOCKED, 1);
            if name == "h_hack" && xp > 0 {
                p.push_i(xp);
                p.host(hw::GIVE_EXP_POINTS);
            }
            p.call_stmt("n_1");
            self.asm.add(p);
        }
    }

    // ---- notes -------------------------------------------------------------

    /// A note's text as one reply (its first 899 bytes are shown).
    pub fn note_text(&self, note: &SimId) -> Option<String> {
        let note = self.ctx.module.notes.get(note)?;
        match &note.body {
            crate::ir::NoteBody::Text(t) => Some(self.text(*t)),
            crate::ir::NoteBody::Voice { topic: Some(topic) } => {
                let text = self.topic_text(topic);
                (!text.is_empty()).then_some(text)
            }
            _ => note.title.map(|t| self.text(t)),
        }
    }

    /// A topic's responses joined, for voice notes (spec 9.2).
    pub fn topic_text(&self, topic: &SimId) -> String {
        let Some(topic) = self.ctx.module.topics.get(topic) else {
            return String::new();
        };
        let mut lines = Vec::new();
        for info in &topic.infos {
            if let Some(info) = self.ctx.module.infos.get(info) {
                for response in &info.responses {
                    lines.push(self.text(response.text));
                }
            }
        }
        lines.join(" ")
    }

    /// The note program: picking the item up adds the note.
    pub fn note_program(&mut self, note: &SimId) {
        self.scope.section = Some(Section::Terminals);
        let mut p = Proc::new("pickup_p_proc");
        self.add_note(&mut p, note);
        self.asm.add(p);
        self.count_in(Section::Terminals, Outcome::Compiled, "");
    }
}
