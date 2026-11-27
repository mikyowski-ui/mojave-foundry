//! The package runner of a critter program (spec 7.2 to 7.5), from the
//! actor's [`crate::ir::ActorRun`] (task T6): `pk_tick`, called from
//! `critter_p_proc`, selects a slot every 300 ticks or when `L_EVP` is set,
//! runs the old slot's end and change events and the new one's begin
//! event on a change, then the selected slot's behaviour on the current
//! map; a hostile actor attacks the player on sight (`hostile_tick`).

use super::asm::Proc;
use super::generator::{Gen, Section, locals};
use super::words::{self as hw};
use crate::ir::{
    Anchor, Event, PackageEvent, Place, RunSlot, RunTarget, Runner, Schedule, SimId, Stmt,
};
use crate::lowered::Outcome;

impl Gen<'_, '_> {
    fn run_slots(&self) -> Vec<RunSlot> {
        self.actor
            .as_ref()
            .and_then(|b| self.ctx.module.runs.get(b))
            .map(|r| r.slots.clone())
            .unwrap_or_default()
    }

    /// Whether the actor attacks the player on sight (spec 7.5).
    pub fn hostile(&self) -> bool {
        self.actor
            .as_ref()
            .and_then(|b| self.ctx.module.runs.get(b))
            .is_some_and(|r| r.combat.hostile)
    }

    /// `obj_can_see_obj(self, player)` (its own perception test) starts
    /// combat with `attack_setup` (E23).
    pub fn hostile_tick(&mut self, p: &mut Proc) {
        let skip = p.label();
        p.branch_false(&skip, |p| {
            p.host(hw::SELF_OBJ);
            p.host(hw::DUDE_OBJ);
            p.host(hw::OBJ_CAN_SEE_OBJ);
        });
        p.host(hw::SELF_OBJ);
        p.host(hw::DUDE_OBJ);
        p.host(hw::ATTACK_SETUP);
        p.place(&skip);
        self.count_in(Section::Packages, Outcome::Compiled, "");
    }

    /// Jumps to `label` unless the schedule holds now (spec 7.2).
    fn schedule_false(&mut self, p: &mut Proc, schedule: &Schedule, label: &str) {
        if schedule.hour >= 0 {
            let duration = if schedule.duration_hours <= 0 {
                24
            } else {
                schedule.duration_hours.min(24)
            };
            let hour = i32::from(schedule.hour);
            p.branch_false(label, |p| {
                p.host(hw::GAME_TIME_HOUR);
                p.push_i(100);
                p.binary("div");
                p.push_i(24 - hour);
                p.binary("add");
                p.push_i(24);
                p.binary("mod");
                p.push_i(duration);
                p.binary("lt");
            });
        }
        if schedule.month >= 0 {
            let month = i32::from(schedule.month) + 1;
            p.branch_false(label, |p| {
                p.host(hw::GET_MONTH);
                p.push_i(month);
                p.binary("eq");
            });
        }
        if schedule.day_of_week >= 0 {
            let day = i32::from(schedule.day_of_week);
            p.branch_false(label, |p| {
                p.host(hw::GAME_TIME);
                p.push_i(864_000);
                p.binary("div");
                p.push_i(7);
                p.binary("mod");
                p.push_i(day);
                p.binary("eq");
            });
        }
        if schedule.date > 0 {
            let date = i32::from(schedule.date);
            p.branch_false(label, |p| {
                p.host(hw::GET_DAY);
                p.push_i(date);
                p.binary("eq");
            });
        }
    }

    /// The event scripts and blocks of one package event.
    fn package_event(&mut self, p: &mut Proc, package: &SimId, event: PackageEvent) {
        let events = self
            .ctx
            .module
            .packages
            .get(package)
            .map(|pk| pk.events.clone())
            .unwrap_or_default();
        for (e, script) in events {
            if e != event {
                continue;
            }
            let body = script
                .as_ref()
                .and_then(|s| self.ctx.module.scripts.get(s))
                .and_then(|s| s.blocks.first())
                .map(|b| b.body.clone());
            if let Some(body) = body {
                self.block(p, &body);
            }
        }
        let script = self
            .actor
            .as_ref()
            .and_then(|b| self.ctx.module.actors.get(b))
            .and_then(|a| a.script.clone());
        let blocks: Vec<Vec<Stmt>> = script
            .as_ref()
            .and_then(|s| self.ctx.module.scripts.get(s))
            .map(|s| {
                s.blocks
                    .iter()
                    .filter(|b| match (&b.event, event) {
                        (Event::OnPackageStart(f), PackageEvent::Begin)
                        | (Event::OnPackageEnd(f), PackageEvent::End)
                        | (Event::OnPackageChange(f), PackageEvent::Change) => {
                            f.as_ref().is_none_or(|f| *f == package.form)
                        }
                        _ => false,
                    })
                    .map(|b| b.body.clone())
                    .collect()
            })
            .unwrap_or_default();
        for body in blocks {
            self.block(p, &body);
        }
    }

    /// A switch on the value `value` pushes, over the slots (1-based).
    fn slot_switch(
        &mut self,
        p: &mut Proc,
        value: impl Fn(&mut Proc),
        slots: &[RunSlot],
        mut body: impl FnMut(&mut Self, &mut Proc, &RunSlot),
    ) {
        let end = p.label();
        for (i, slot) in slots.iter().enumerate() {
            let mark = p.mark();
            let next = p.label();
            p.branch_false(&next, |p| {
                value(p);
                p.push_i(i as i32 + 1);
                p.binary("eq");
            });
            body(self, p, slot);
            p.jump(&end);
            p.place(&next);
            p.release(mark);
        }
        p.place(&end);
    }

    /// Walks toward the hex in the temporary `goal` when not there and not
    /// busy.
    fn walk_to(p: &mut Proc, goal: u32, run: i32) {
        let skip = p.label();
        p.branch_false(&skip, |p| {
            p.host(hw::SELF_OBJ);
            p.host(hw::TILE_NUM);
            p.fetch_local(goal);
            p.binary("ne");
        });
        p.branch_false(&skip, |p| {
            p.host(hw::SELF_OBJ);
            p.host(hw::ANIM_BUSY);
            p.unary("not");
        });
        p.host(hw::SELF_OBJ);
        p.fetch_local(goal);
        p.push_i(run);
        p.host(hw::ANIMATE_MOVE_OBJ_TO_TILE);
        p.place(&skip);
    }

    /// Pushes the anchor's hex, or -1 when it has none (stand).
    fn anchor_hex(&mut self, p: &mut Proc, anchor: &Anchor) {
        match anchor {
            Anchor::Hex(at) => p.push_i(at.hex as i32),
            Anchor::Editor => {
                p.push_i(locals::ANCHOR as i32);
                p.host(hw::LOCAL_VAR);
            }
            Anchor::Current => p.push_i(-1),
            Anchor::Ref(r) => {
                let static_hex = self
                    .ctx
                    .numbers
                    .slots
                    .get(r)
                    .filter(|s| !s.mobile)
                    .and_then(|s| s.placed.as_ref())
                    .map(|o| o.hex);
                match static_hex {
                    Some(hex) => p.push_i(hex as i32),
                    None => {
                        let t = p.temp();
                        self.slot_object(p, r);
                        p.store_local(t);
                        let none = p.label();
                        let end = p.label();
                        p.branch_false(&none, |p| p.fetch_local(t));
                        p.fetch_local(t);
                        p.host(hw::TILE_NUM);
                        p.jump(&end);
                        p.place(&none);
                        p.push_i(-1);
                        p.place(&end);
                    }
                }
            }
        }
    }

    fn place_behaviour(&mut self, p: &mut Proc, slot: &RunSlot, place: &Place) {
        let run = i32::from(slot.run);
        match slot.runner {
            Runner::Travel | Runner::Guard => {
                let goal = p.temp();
                self.anchor_hex(p, &place.anchor);
                p.store_local(goal);
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.fetch_local(goal);
                    p.push_i(0);
                    p.binary("ge");
                });
                Self::walk_to(p, goal, run);
                p.place(&skip);
            }
            Runner::Sandbox => {
                // Every 100 to 300 ticks (L_PHASE), one of the candidates.
                let skip = p.label();
                p.branch_false(&skip, |p| {
                    p.host(hw::GAME_TIME);
                    p.push_i(locals::PHASE as i32);
                    p.host(hw::LOCAL_VAR);
                    p.binary("ge");
                });
                p.branch_false(&skip, |p| {
                    p.host(hw::SELF_OBJ);
                    p.host(hw::ANIM_BUSY);
                    p.unary("not");
                });
                p.push_i(locals::PHASE as i32);
                p.host(hw::GAME_TIME);
                p.push_i(100);
                p.push_i(300);
                p.host(hw::RANDOM);
                p.binary("add");
                p.host(hw::SET_LOCAL_VAR);
                let goal = p.temp();
                if place.candidates.is_empty() {
                    self.anchor_hex(p, &place.anchor);
                    p.store_local(goal);
                    self.count_in(
                        Section::Packages,
                        Outcome::Fallback,
                        "sandbox with no walkable candidates: stays at its anchor",
                    );
                } else {
                    let draw = p.temp();
                    p.push_i(1);
                    p.push_i(place.candidates.len() as i32);
                    p.host(hw::RANDOM);
                    p.store_local(draw);
                    for (i, at) in place.candidates.iter().enumerate() {
                        let next = p.label();
                        p.branch_false(&next, |p| {
                            p.fetch_local(draw);
                            p.push_i(i as i32 + 1);
                            p.binary("eq");
                        });
                        p.push_i(at.hex as i32);
                        p.store_local(goal);
                        p.place(&next);
                    }
                }
                p.branch_false(&skip, |p| {
                    p.fetch_local(goal);
                    p.push_i(0);
                    p.binary("ge");
                });
                p.host(hw::SELF_OBJ);
                p.fetch_local(goal);
                p.push_i(run);
                p.host(hw::ANIMATE_MOVE_OBJ_TO_TILE);
                p.place(&skip);
            }
            Runner::Follow | Runner::Wait | Runner::None => {}
        }
    }

    fn follow(&mut self, p: &mut Proc, target: &Option<RunTarget>) {
        let t = p.temp();
        match target {
            Some(RunTarget::Ref(r)) => self.slot_object(p, r),
            _ => p.host(hw::DUDE_OBJ),
        }
        p.store_local(t);
        let skip = p.label();
        p.branch_false(&skip, |p| p.fetch_local(t));
        p.branch_false(&skip, |p| {
            p.host(hw::SELF_OBJ);
            p.fetch_local(t);
            p.host(hw::TILE_DISTANCE_OBJS);
            p.push_i(2);
            p.binary("gt");
        });
        p.branch_false(&skip, |p| {
            p.host(hw::SELF_OBJ);
            p.host(hw::ANIM_BUSY);
            p.unary("not");
        });
        p.host(hw::SELF_OBJ);
        p.fetch_local(t);
        p.host(hw::TILE_NUM);
        p.host(hw::SELF_OBJ);
        p.fetch_local(t);
        p.host(hw::TILE_DISTANCE_OBJS);
        p.push_i(7);
        p.binary("gt");
        p.host(hw::ANIMATE_MOVE_OBJ_TO_TILE);
        p.place(&skip);
    }

    fn behave(&mut self, p: &mut Proc, slot: &RunSlot) {
        self.scope.owner = slot.package.form.to_string();
        if let Some(reason) = &slot.fallback {
            self.count_in(Section::Packages, Outcome::Fallback, reason);
        } else {
            self.count_in(Section::Packages, Outcome::Compiled, "");
        }
        if slot.runner == Runner::Follow {
            self.follow(p, &slot.target);
            return;
        }
        for place in &slot.places {
            let Some(map) = self.ctx.numbers.map_number(&place.map) else {
                continue;
            };
            let skip = p.label();
            p.branch_false(&skip, |p| {
                p.host(hw::CUR_MAP_INDEX);
                p.push_i(map as i32);
                p.binary("eq");
            });
            self.place_behaviour(p, slot, place);
            p.place(&skip);
        }
    }

    /// The runner's tick procedure `pk_tick`, when the actor has slots.
    pub fn runner(&mut self) -> bool {
        let slots = self.run_slots();
        if slots.is_empty() {
            return false;
        }
        self.scope.section = Some(Section::Packages);
        let mut p = Proc::new("pk_tick");
        self.scope.temps.clear();
        let evaluate = p.label();
        let behave = p.label();
        let by_time = p.label();
        p.branch_false(&by_time, |p| {
            p.push_i(locals::EVP as i32);
            p.host(hw::LOCAL_VAR);
        });
        p.jump(&evaluate);
        p.place(&by_time);
        p.branch_false(&behave, |p| {
            p.host(hw::GAME_TIME);
            p.push_i(locals::NEXT as i32);
            p.host(hw::LOCAL_VAR);
            p.binary("ge");
        });
        p.place(&evaluate);
        self.set_local(&mut p, locals::EVP, 0);
        self.set_local_with(&mut p, locals::NEXT, |_, p| {
            p.host(hw::GAME_TIME);
            p.push_i(300);
            p.binary("add");
        });
        let selected = p.temp();
        let chosen = p.label();
        let none_scripted = p.label();
        p.branch_false(&none_scripted, |p| {
            p.push_i(locals::SCRIPTPKG as i32);
            p.host(hw::LOCAL_VAR);
        });
        p.push_i(locals::SCRIPTPKG as i32);
        p.host(hw::LOCAL_VAR);
        p.store_local(selected);
        p.jump(&chosen);
        p.place(&none_scripted);
        for (i, slot) in slots.iter().enumerate() {
            self.scope.owner = slot.package.form.to_string();
            let mark = p.mark();
            let next = p.label();
            self.schedule_false(&mut p, &slot.schedule, &next);
            self.cond_false(&mut p, &slot.condition, &next);
            p.push_i(i as i32 + 1);
            p.store_local(selected);
            p.jump(&chosen);
            p.place(&next);
            p.release(mark);
        }
        p.place(&chosen);
        let same = p.label();
        p.branch_false(&same, |p| {
            p.fetch_local(selected);
            p.push_i(locals::PKG as i32);
            p.host(hw::LOCAL_VAR);
            p.binary("ne");
        });
        self.slot_switch(
            &mut p,
            |p| {
                p.push_i(locals::PKG as i32);
                p.host(hw::LOCAL_VAR);
            },
            &slots,
            |g, p, slot| {
                g.package_event(p, &slot.package, PackageEvent::End);
                g.package_event(p, &slot.package, PackageEvent::Change);
            },
        );
        self.set_local_with(&mut p, locals::PKG, |_, p| p.fetch_local(selected));
        self.set_local(&mut p, locals::PHASE, 0);
        self.set_local(&mut p, locals::STUCK, 0);
        self.slot_switch(
            &mut p,
            |p| p.fetch_local(selected),
            &slots,
            |g, p, slot| g.package_event(p, &slot.package, PackageEvent::Begin),
        );
        p.place(&same);
        p.place(&behave);
        self.slot_switch(
            &mut p,
            |p| {
                p.push_i(locals::PKG as i32);
                p.host(hw::LOCAL_VAR);
            },
            &slots,
            |g, p, slot| g.behave(p, slot),
        );
        self.asm.add(p);
        true
    }
}
