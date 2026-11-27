//! One program per [`ProgramKey`] (spec 5.5, 5.6, 5.8, 8.3, 9, 10.1):
//! which procedures it has and what each runs.

use std::collections::BTreeMap;

use super::asm::Proc;
use super::generator::{Gen, Section, locals};
use super::words::{self as hw};
use crate::diag::{Category, Diag};
use crate::ir::{Block, Event, FormKey, PlacedKind, SimId};
use crate::lowered::Outcome;
use crate::numbering::{ProgramKey, RefSlot};

fn event_name(event: &Event) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Object(map) => map.keys().next().cloned(),
            _ => None,
        })
        .unwrap_or_else(|| format!("{event:?}"))
}

impl Gen<'_, '_> {
    fn script_blocks(&self, script: Option<&SimId>) -> Vec<Block> {
        script
            .and_then(|s| self.ctx.module.scripts.get(s))
            .map(|s| s.blocks.clone())
            .unwrap_or_default()
    }

    fn blocks_of(blocks: &[Block], f: impl Fn(&Event) -> bool) -> Vec<Vec<crate::ir::Stmt>> {
        blocks
            .iter()
            .filter(|b| f(&b.event))
            .map(|b| b.body.clone())
            .collect()
    }

    fn run_blocks(&mut self, p: &mut Proc, bodies: &[Vec<crate::ir::Stmt>]) {
        let saved = self.scope.section;
        self.scope.section = Some(Section::Scripts);
        for body in bodies {
            self.block(p, body);
            self.count_in(Section::Scripts, Outcome::Compiled, "block");
        }
        self.scope.section = saved;
    }

    /// Counts the blocks no procedure of the program runs.
    fn drop_blocks(&mut self, blocks: &[Block], handled: impl Fn(&Event) -> bool, why: &str) {
        for block in blocks {
            if !handled(&block.event) {
                let reason = format!("block {}: {why}", event_name(&block.event));
                self.count_in(Section::Scripts, Outcome::Dropped, &reason);
            }
        }
    }

    /// The slots of this actor's placements.
    fn own_slots(&self) -> Vec<(FormKey, RefSlot)> {
        self.ctx
            .numbers
            .slots
            .iter()
            .filter(|(_, s)| {
                s.placed
                    .as_ref()
                    .is_some_and(|o| self.program_owns(o) && o.kind == PlacedKind::Critter)
            })
            .map(|(f, s)| (f.clone(), s.clone()))
            .collect()
    }

    /// `L_SLOT` from the placed hex, on the first run (spec 5.8).
    fn find_slot(&mut self, p: &mut Proc) {
        let done = p.label();
        for (_, slot) in self.own_slots() {
            let Some(placed) = &slot.placed else {
                continue;
            };
            let next = p.label();
            let (map, hex, elevation) = (placed.map_number, placed.hex, placed.elevation);
            p.branch_false(&next, |p| {
                p.host(hw::CUR_MAP_INDEX);
                p.push_i(map as i32);
                p.binary("eq");
            });
            p.branch_false(&next, |p| {
                p.host(hw::SELF_OBJ);
                p.host(hw::TILE_NUM);
                p.push_i(hex as i32);
                p.binary("eq");
            });
            p.branch_false(&next, |p| {
                p.host(hw::SELF_OBJ);
                p.host(hw::ELEVATION);
                p.push_i(i32::from(elevation));
                p.binary("eq");
            });
            self.set_local(p, locals::SLOT, slot.k as i32);
            p.jump(&done);
            p.place(&next);
        }
        p.place(&done);
    }

    /// `r<k> = self_obj` for this instance's slot (spec 5.8, mobile).
    fn store_self(&mut self, p: &mut Proc) {
        for (_, slot) in self.own_slots() {
            let Some(placed) = &slot.placed else {
                continue;
            };
            let name = slot.name();
            self.asm.name(&name);
            let next = p.label();
            let map = placed.map_number;
            p.branch_false(&next, |p| {
                p.push_i(locals::SLOT as i32);
                p.host(hw::LOCAL_VAR);
                p.push_i(slot.k as i32);
                p.binary("eq");
            });
            p.branch_false(&next, |p| {
                p.host(hw::CUR_MAP_INDEX);
                p.push_i(map as i32);
                p.binary("eq");
            });
            p.host(hw::SELF_OBJ);
            p.push_n(&name);
            p.core("store_external", 2, 0);
            p.place(&next);
        }
    }

    fn pending_group_procs(&mut self) {
        let mut done = std::collections::BTreeSet::new();
        loop {
            let todo: Vec<String> = self
                .pending_procs
                .iter()
                .filter(|n| !done.contains(*n))
                .cloned()
                .collect();
            if todo.is_empty() {
                break;
            }
            for name in todo {
                done.insert(name.clone());
                let index: usize = name
                    .strip_prefix("grp_")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                let Some(group) = self.ctx.module.groups.keys().nth(index - 1).cloned() else {
                    continue;
                };
                let mut p = Proc::new(&name);
                self.apply_group(&mut p, &group, None);
                self.asm.add(p);
            }
        }
    }

    fn init_guard(&mut self, p: &mut Proc, body: impl FnOnce(&mut Self, &mut Proc)) {
        let skip = p.label();
        p.branch_false(&skip, |p| {
            p.push_i(locals::INIT as i32);
            p.host(hw::LOCAL_VAR);
            p.push_i(0);
            p.binary("eq");
        });
        self.set_local(p, locals::INIT, 1);
        body(self, p);
        p.place(&skip);
    }

    /// An actor base's program (spec 4, 5.5, 5.6, 7).
    pub fn critter_program(&mut self, base: &FormKey) {
        self.actor = Some(base.clone());
        self.scope.has_self = true;
        let actor = self.ctx.module.actors.get(base).cloned();
        let script = actor.as_ref().and_then(|a| a.script.clone());
        self.scope.owner = actor
            .as_ref()
            .and_then(|a| a.editor_id.clone())
            .unwrap_or_else(|| base.to_string());
        self.local_count = locals::CRITTER;
        if let Some(script) = &script {
            self.local_count = self.declare_script_locals(script, locals::CRITTER);
        }
        let plan = self.plan_dialogue(base);
        self.dialogue = (!plan.is_empty()).then(|| plan.clone());
        let blocks = self.script_blocks(script.as_ref());

        // start
        let mut p = Proc::new("start");
        self.init_guard(&mut p, |g, p| {
            g.find_slot(p);
            g.set_local_with(p, locals::ANCHOR, |_, p| {
                p.host(hw::SELF_OBJ);
                p.host(hw::TILE_NUM);
            });
            g.set_local_with(p, locals::LAST_GM, |_, p| p.host(hw::GAME_TIME));
            g.set_local(p, locals::EVP, 1);
            // The faction team (spec 7.5), once per critter, before its
            // first turn: a later change (the party, a faction switch) is
            // kept.
            g.set_team(p);
        });
        self.store_self(&mut p);
        self.apply_self_groups(&mut p);
        self.asm.add(p);

        // map_enter_p_proc
        let mut p = Proc::new("map_enter_p_proc");
        self.scope.temps.clear();
        self.store_self(&mut p);
        self.set_local(&mut p, locals::EVP, 1);
        self.apply_self_groups(&mut p);
        let on_load = Self::blocks_of(&blocks, |e| *e == Event::OnLoad);
        self.run_blocks(&mut p, &on_load);
        self.asm.add(p);

        // talk_p_proc
        let activate = Self::blocks_of(&blocks, |e| *e == Event::OnActivate);
        if !activate.is_empty() || self.dialogue.is_some() {
            let mut p = Proc::new("talk_p_proc");
            self.scope.temps.clear();
            let skip = p.label();
            if !activate.is_empty() {
                let flag = p.temp();
                self.scope.activation = Some(flag);
                self.run_blocks(&mut p, &activate);
                self.scope.activation = None;
                p.branch_false(&skip, |p| p.fetch_local(flag));
            }
            if let Some(plan) = self.dialogue.clone() {
                self.talk(&mut p, &plan);
            }
            p.place(&skip);
            self.asm.add(p);
        }

        // critter_p_proc
        let has_runner = self.runner();
        let hostile = self.hostile();
        let game_mode = Self::blocks_of(&blocks, |e| *e == Event::GameMode);
        let combat_end = Self::blocks_of(&blocks, |e| *e == Event::OnCombatEnd);
        if has_runner || hostile || !game_mode.is_empty() || !combat_end.is_empty() {
            let mut p = Proc::new("critter_p_proc");
            self.scope.temps.clear();
            if hostile {
                self.hostile_tick(&mut p);
            }
            if has_runner {
                p.call_stmt("pk_tick");
            }
            if !combat_end.is_empty() {
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.push_i(locals::FLAGS as i32);
                    p.host(hw::LOCAL_VAR);
                    p.push_i(1);
                    p.binary("band");
                });
                self.clear_local_bit(&mut p, locals::FLAGS, 1);
                self.run_blocks(&mut p, &combat_end);
                p.place(&skip);
            }
            if !game_mode.is_empty() {
                // At most once per 10 ticks (spec 5.6).
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.host(hw::GAME_TIME);
                    p.push_i(locals::LAST_GM as i32);
                    p.host(hw::LOCAL_VAR);
                    p.binary("sub");
                    p.push_i(10);
                    p.binary("ge");
                });
                let seconds = p.temp();
                p.host(hw::GAME_TIME);
                p.push_i(locals::LAST_GM as i32);
                p.host(hw::LOCAL_VAR);
                p.binary("sub");
                p.push_f(10.0);
                p.binary("div");
                p.store_local(seconds);
                self.set_local_with(&mut p, locals::LAST_GM, |_, p| p.host(hw::GAME_TIME));
                self.scope.seconds = Some(seconds);
                self.run_blocks(&mut p, &game_mode);
                self.scope.seconds = None;
                p.place(&skip);
            }
            self.asm.add(p);
        }

        // combat_p_proc
        let start_combat = Self::blocks_of(&blocks, |e| *e == Event::OnStartCombat);
        if !start_combat.is_empty() || !combat_end.is_empty() {
            let mut p = Proc::new("combat_p_proc");
            self.scope.temps.clear();
            let skip = p.label();
            p.branch_false(&skip, |p| {
                p.host(hw::FIXED_PARAM);
                p.push_i(4);
                p.binary("eq");
            });
            p.branch_false(&skip, |p| {
                p.push_i(locals::FLAGS as i32);
                p.host(hw::LOCAL_VAR);
                p.push_i(1);
                p.binary("band");
                p.unary("not");
            });
            self.set_local_with(&mut p, locals::FLAGS, |_, p| {
                p.push_i(locals::FLAGS as i32);
                p.host(hw::LOCAL_VAR);
                p.push_i(1);
                p.binary("bor");
            });
            self.run_blocks(&mut p, &start_combat);
            p.place(&skip);
            self.asm.add(p);
        }

        // destroy_p_proc
        let death = Self::blocks_of(&blocks, |e| matches!(e, Event::OnDeath { .. }));
        let counter = self
            .ctx
            .numbers
            .global(&crate::numbering::derived::dead_count(base));
        let dead: Vec<(u32, u32)> = self
            .own_slots()
            .iter()
            .filter_map(|(form, slot)| {
                self.ctx
                    .numbers
                    .global(&crate::numbering::derived::dead(form))
                    .map(|g| (slot.k, g))
            })
            .collect();
        if !death.is_empty() || counter.is_some() || !dead.is_empty() {
            let mut p = Proc::new("destroy_p_proc");
            self.scope.temps.clear();
            for (k, g) in dead {
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.push_i(locals::SLOT as i32);
                    p.host(hw::LOCAL_VAR);
                    p.push_i(k as i32);
                    p.binary("eq");
                });
                self.set_global(&mut p, g, 1);
                p.place(&skip);
            }
            if let Some(g) = counter {
                self.set_global_with(&mut p, g, |gw, p| {
                    gw.get_global(p, g);
                    p.push_i(1);
                    p.binary("add");
                });
            }
            self.run_blocks(&mut p, &death);
            self.asm.add(p);
        }

        // damage_p_proc
        let hit = Self::blocks_of(&blocks, |e| matches!(e, Event::OnHit { .. }));
        if !hit.is_empty() {
            let mut p = Proc::new("damage_p_proc");
            self.scope.temps.clear();
            self.run_blocks(&mut p, &hit);
            self.asm.add(p);
        }

        self.drop_blocks(
            &blocks,
            |e| {
                matches!(
                    e,
                    Event::OnLoad
                        | Event::OnActivate
                        | Event::GameMode
                        | Event::OnCombatEnd
                        | Event::OnStartCombat
                        | Event::OnDeath { .. }
                        | Event::OnHit { .. }
                        | Event::OnPackageStart(_)
                        | Event::OnPackageEnd(_)
                        | Event::OnPackageChange(_)
                )
            },
            "not run by a critter program in Pass 2",
        );
        if let Some(plan) = self.dialogue.clone() {
            self.dialogue_procs(&plan);
        }
    }

    fn clear_local_bit(&mut self, p: &mut Proc, slot: u32, mask: i32) {
        self.set_local_with(p, slot, |_, p| {
            p.push_i(slot as i32);
            p.host(hw::LOCAL_VAR);
            p.push_i(!mask);
            p.binary("band");
        });
    }

    /// A non-actor base's program: its script, and its lock (spec 5.5,
    /// 5.6, 9.3).
    pub fn object_program(&mut self, base: &FormKey, lock: &Option<(u8, Option<FormKey>)>) {
        self.scope.has_self = true;
        let script = self.ctx.facts.base_scripts.get(base).cloned();
        self.scope.owner = script
            .as_ref()
            .and_then(|s| self.ctx.module.scripts.get(s))
            .map(|s| s.editor_id.clone())
            .unwrap_or_else(|| base.to_string());
        self.local_count = locals::OBJECT;
        if let Some(script) = &script {
            self.local_count = self.declare_script_locals(script, locals::OBJECT);
        }
        let blocks = self.script_blocks(script.as_ref());
        let game_mode = Self::blocks_of(&blocks, |e| *e == Event::GameMode);

        let mut p = Proc::new("start");
        self.init_guard(&mut p, |_, _| {});
        self.asm.add(p);

        let on_load = Self::blocks_of(&blocks, |e| *e == Event::OnLoad);
        if !on_load.is_empty() || !game_mode.is_empty() {
            let mut p = Proc::new("map_enter_p_proc");
            self.scope.temps.clear();
            self.run_blocks(&mut p, &on_load);
            if !game_mode.is_empty() {
                p.host(hw::SELF_OBJ);
                p.push_i(10);
                p.push_i(1);
                p.host(hw::ADD_TIMER_EVENT);
            }
            self.asm.add(p);
        }
        if !game_mode.is_empty() {
            let mut p = Proc::new("timed_event_p_proc");
            self.scope.temps.clear();
            let skip = p.label();
            p.branch_false(&skip, |p| {
                p.host(hw::FIXED_PARAM);
                p.push_i(1);
                p.binary("eq");
            });
            self.scope.seconds = None;
            self.scope.seconds_const = 1.0;
            self.run_blocks(&mut p, &game_mode);
            p.host(hw::SELF_OBJ);
            p.push_i(10);
            p.push_i(1);
            p.host(hw::ADD_TIMER_EVENT);
            p.place(&skip);
            self.asm.add(p);
        }

        let activate = Self::blocks_of(&blocks, |e| *e == Event::OnActivate);
        if lock.is_some() || !activate.is_empty() {
            let mut p = Proc::new("use_p_proc");
            self.scope.temps.clear();
            if let Some((_, key)) = lock {
                self.lock_use(&mut p, key);
            }
            if !activate.is_empty() {
                let flag = p.temp();
                self.scope.activation = Some(flag);
                self.run_blocks(&mut p, &activate);
                self.scope.activation = None;
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.fetch_local(flag);
                    p.unary("not");
                });
                p.host(hw::SCRIPT_OVERRIDES);
                p.place(&skip);
            }
            self.asm.add(p);
        }
        if let Some((level, _)) = lock {
            let mut p = Proc::new("use_skill_on_p_proc");
            self.scope.temps.clear();
            self.lock_skill(&mut p, *level);
            self.asm.add(p);
        }
        let death = Self::blocks_of(&blocks, |e| matches!(e, Event::OnDeath { .. }));
        if !death.is_empty() {
            let mut p = Proc::new("destroy_p_proc");
            self.scope.temps.clear();
            self.run_blocks(&mut p, &death);
            self.asm.add(p);
        }
        let hit = Self::blocks_of(&blocks, |e| matches!(e, Event::OnHit { .. }));
        if !hit.is_empty() {
            let mut p = Proc::new("damage_p_proc");
            self.scope.temps.clear();
            self.run_blocks(&mut p, &hit);
            self.asm.add(p);
        }
        self.drop_blocks(
            &blocks,
            |e| {
                matches!(
                    e,
                    Event::OnLoad
                        | Event::OnActivate
                        | Event::GameMode
                        | Event::OnDeath { .. }
                        | Event::OnHit { .. }
                )
            },
            "not run by an object program in Pass 2 (trigger volumes have no geometry in the IR)",
        );
    }

    /// A terminal's program (spec 9.1).
    pub fn terminal_program(&mut self, terminal: &SimId) {
        self.scope.has_self = true;
        self.scope.owner = terminal.form.to_string();
        self.local_count = locals::UNLOCKED + 1;
        let mut p = Proc::new("start");
        self.init_guard(&mut p, |_, _| {});
        self.asm.add(p);
        self.terminal(terminal);
    }

    /// A note item's program (spec 9.2).
    pub fn note_item_program(&mut self, note: &SimId) {
        self.scope.has_self = true;
        self.scope.owner = note.form.to_string();
        self.local_count = 0;
        let p = Proc::new("start");
        self.asm.add(p);
        self.note_program(note);
    }

    /// A map's script (spec 5.8, 8.3): exports the map's slots, resolves the
    /// static ones, and applies the groups' reference table.
    pub fn map_program(&mut self, map: &str) {
        self.scope.has_self = false;
        self.scope.owner = map.to_owned();
        self.scope.section = Some(Section::State);
        self.local_count = 0;
        let number = self.ctx.numbers.map_number(map).unwrap_or(0);
        let slots: Vec<RefSlot> = self
            .ctx
            .numbers
            .slots
            .values()
            .filter(|s| s.placed.as_ref().is_some_and(|o| o.map == map))
            .cloned()
            .collect();
        for slot in &slots {
            self.asm.export(&slot.name());
        }
        let mut seen: BTreeMap<(u32, u8, u32), u32> = BTreeMap::new();
        let mut statics = Vec::new();
        for slot in &slots {
            if slot.mobile {
                continue;
            }
            let Some(placed) = &slot.placed else {
                continue;
            };
            let Some(pid) = placed.pid else {
                self.count_in(
                    Section::State,
                    Outcome::Dropped,
                    "static reference with no PID: its slot stays none",
                );
                continue;
            };
            let key = (placed.hex, placed.elevation, pid);
            if let Some(other) = seen.get(&key) {
                self.diags.push(
                    Diag::error(
                        Category::RefOutsideSlice,
                        format!(
                            "slots r{other} and r{} have one PID on one hex and elevation of map {map}",
                            slot.k
                        ),
                    )
                    .about(map),
                );
                continue;
            }
            seen.insert(key, slot.k);
            statics.push((slot.name(), placed.hex, placed.elevation, pid));
        }
        let groups: Vec<SimId> = self.ctx.module.groups.keys().cloned().collect();
        for name in ["start", "map_enter_p_proc", "map_update_p_proc"] {
            let mut p = Proc::new(name);
            if name != "map_update_p_proc" {
                for (var, hex, elevation, pid) in &statics {
                    p.push_i(*hex as i32);
                    p.push_i(i32::from(*elevation));
                    p.push_i(*pid as i32);
                    p.host(hw::TILE_CONTAINS_PID_OBJ);
                    p.push_n(var);
                    p.core("store_external", 2, 0);
                }
            }
            for group in &groups {
                self.apply_group(&mut p, group, Some(number));
            }
            self.asm.add(p);
        }
    }

    /// The quest dispatcher's program.
    pub fn dispatcher_program(&mut self) {
        self.local_count = 0;
        self.scope.owner = "mfquest".to_owned();
        self.dispatcher();
    }

    /// Generates the program of `self.key`.
    pub fn generate(&mut self) {
        match self.key.clone() {
            ProgramKey::Map(map) => self.map_program(&map),
            ProgramKey::Critter(base) => self.critter_program(&base),
            ProgramKey::Object { base, lock } => self.object_program(&base, &lock),
            ProgramKey::Note(note) => self.note_item_program(&note),
            ProgramKey::Terminal(terminal) => self.terminal_program(&terminal),
            ProgramKey::Dispatcher => self.dispatcher_program(),
        }
        // Every kind: the group appliers its statements asked for
        // (`Enable`/`Disable` of a group parent calls `grp_<n>`).
        self.pending_group_procs();
    }
}
