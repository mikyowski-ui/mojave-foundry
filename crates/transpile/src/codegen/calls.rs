//! Function calls: the statement lowering of spec 5.9 and the condition
//! functions of spec 4.8, on Fallout 2 words. A call the table drops, or
//! one with nothing to lower it to, is a counted fallback: as a statement
//! it emits nothing, as a value it reads its fallback (0 unless the table
//! says otherwise).

use super::asm::Proc;
use super::generator::{Gen, Section, Ty, locals};
use super::words::{self as hw};
use crate::ir::{
    ActorValue, Call, CmpOp, Expr, FormKey, Func, RefExpr, SimId, SimKind, Stat, quest_sub,
};
use crate::lowered::Outcome;
use crate::numbering::derived;

/// The quest a call's first argument names.
fn quest_arg(call: &Call) -> Option<&FormKey> {
    match call.args.first() {
        Some(Expr::Form(form)) => Some(form),
        _ => None,
    }
}

fn form_arg(call: &Call, index: usize) -> Option<&FormKey> {
    match call.args.get(index) {
        Some(Expr::Form(form)) => Some(form),
        Some(Expr::Ref(RefExpr::Placed(form))) => Some(form),
        _ => None,
    }
}

fn int_arg(call: &Call, index: usize) -> Option<i32> {
    match call.args.get(index) {
        Some(Expr::Int(v)) => Some(*v),
        Some(Expr::Float(v)) => Some(*v as i32),
        _ => None,
    }
}

impl Gen<'_, '_> {
    /// A quest's stage position: the global word and the bit of `stage`.
    pub fn stage_bit(&self, quest: &FormKey, stage: i32) -> Option<(u32, i32)> {
        let q = self
            .ctx
            .module
            .quests
            .get(&SimId::of(SimKind::Quest, quest))?;
        let index = i16::try_from(stage).ok()?;
        let (word, bit) = crate::quest::stage_bit(q, index)?;
        Some((word, 1 << bit))
    }

    fn quest_global(&mut self, quest: &FormKey, sub: u32) -> Option<u32> {
        self.global(&SimId::quest_state(quest, sub))
    }

    /// `SetStage q s` (spec 6.2): the stage global to the maximum, the
    /// done and pending bits, and the running flag, at once.
    pub fn set_stage(&mut self, p: &mut Proc, quest: &FormKey, stage: &Expr) {
        let Some(stage_g) = self.quest_global(quest, quest_sub::STAGE) else {
            return;
        };
        let t = p.temp();
        self.expr_int(p, stage);
        p.store_local(t);
        // STAGE = max(STAGE, s)
        let skip = p.label();
        p.branch_false(&skip, |p| {
            p.push_i(stage_g as i32);
            p.host(hw::GLOBAL_VAR);
            p.fetch_local(t);
            p.binary("lt");
        });
        p.push_i(stage_g as i32);
        p.fetch_local(t);
        p.host(hw::SET_GLOBAL_VAR);
        p.place(&skip);
        if let Some(flags) = self.quest_global(quest, quest_sub::FLAGS) {
            self.set_bits(p, flags, 1);
        }
        let stages: Vec<i16> = self
            .ctx
            .module
            .quests
            .get(&SimId::of(SimKind::Quest, quest))
            .map(|q| q.stages.iter().map(|s| s.index).collect())
            .unwrap_or_default();
        for index in stages {
            let Some((word, bit)) = self.stage_bit(quest, i32::from(index)) else {
                continue;
            };
            let next = p.label();
            if let Expr::Int(v) = stage {
                if *v != i32::from(index) {
                    continue;
                }
            } else {
                p.branch_false(&next, |p| {
                    p.fetch_local(t);
                    p.push_i(i32::from(index));
                    p.binary("eq");
                });
            }
            if let Some(done) = self.quest_global(quest, quest_sub::DONE + word) {
                self.set_bits(p, done, bit);
            }
            if let Some(pend) = self.quest_global(quest, quest_sub::PEND + word) {
                self.set_bits(p, pend, bit);
            }
            p.place(&next);
        }
    }

    fn objective_global(&mut self, quest: &FormKey, call: &Call) -> Option<u32> {
        let Some(index) = int_arg(call, 1) else {
            self.fallback("objective index is not a literal");
            return None;
        };
        self.global(&SimId::objective(quest, index))
    }

    fn actor_value(&mut self, p: &mut Proc, call: &Call) -> Ty {
        let av = match call.args.first() {
            Some(Expr::Int(v)) => ActorValue(*v as u32),
            _ => {
                self.fallback("actor value is not a constant; read as 0");
                p.push_i(0);
                return Ty::Int;
            }
        };
        match (av.stat(), av.0) {
            (Some(Stat::Special(n)), _) => {
                self.subject(p, &call.on);
                p.push_i(i32::from(n));
                p.host(hw::GET_CRITTER_STAT);
            }
            (Some(Stat::Skill(skill)), _) => {
                self.subject(p, &call.on);
                p.push_i(skill as i32);
                p.host(hw::HAS_SKILL);
            }
            (None, 16) => {
                self.subject(p, &call.on);
                p.push_i(35);
                p.host(hw::GET_CRITTER_STAT);
            }
            (None, 23) => {
                p.push_i(4);
                p.host(hw::GET_PC_STAT);
                if let Some(karma) = self.ctx.numbers.global(&derived::karma()) {
                    self.get_global(p, karma);
                    p.binary("add");
                }
            }
            (None, 25..=30) => p.push_i(100),
            _ => {
                self.fallback(&format!(
                    "actor value {} has no engine read; read as 0",
                    av.0
                ));
                p.push_i(0);
            }
        }
        self.count(Outcome::Compiled, "");
        Ty::Int
    }

    /// Whether `on` is the running object of an actor program for `base`.
    fn on_self_base(&self, on: &Option<RefExpr>) -> Option<&FormKey> {
        match on {
            None | Some(RefExpr::SelfRef) => self.actor.as_ref(),
            _ => None,
        }
    }

    /// A call whose value is used.
    pub fn call_value(&mut self, p: &mut Proc, call: &Call) -> Ty {
        let name = match &call.func {
            Func::Known(id) => id.name(),
            Func::Unsupported { name } => {
                let name = name.clone();
                self.fallback(&format!("{name} is not supported; read as 0"));
                p.push_i(0);
                return Ty::Int;
            }
        };
        let compiled = |g: &mut Self| g.count(Outcome::Compiled, "");
        match name {
            "GetStage" => {
                if let Some(q) = quest_arg(call).cloned()
                    && let Some(g) = self.quest_global(&q, quest_sub::STAGE)
                {
                    self.get_global(p, g);
                    compiled(self);
                } else {
                    p.push_i(0);
                }
            }
            "GetStageDone" => {
                let quest = quest_arg(call).cloned();
                let stage = int_arg(call, 1);
                match (quest, stage) {
                    (Some(q), Some(s)) => match self.stage_bit(&q, s) {
                        Some((word, bit)) => match self.quest_global(&q, quest_sub::DONE + word) {
                            Some(g) => {
                                self.test_bits(p, g, bit);
                                compiled(self);
                            }
                            None => p.push_i(0),
                        },
                        None => {
                            self.count(
                                Outcome::Folded,
                                "GetStageDone of a stage the quest does not have",
                            );
                            p.push_i(0);
                        }
                    },
                    _ => {
                        self.fallback("GetStageDone with a non-literal argument; read as 0");
                        p.push_i(0);
                    }
                }
            }
            "GetQuestRunning" => match quest_arg(call).cloned() {
                Some(q) => match self.quest_global(&q, quest_sub::FLAGS) {
                    Some(g) => {
                        self.test_bits(p, g, 1);
                        compiled(self);
                    }
                    None => p.push_i(0),
                },
                None => p.push_i(0),
            },
            "GetObjectiveDisplayed" | "GetObjectiveCompleted" => {
                let threshold = if name == "GetObjectiveDisplayed" {
                    1
                } else {
                    2
                };
                match quest_arg(call).cloned() {
                    Some(q) => match self.objective_global(&q, call) {
                        Some(g) => {
                            self.get_global(p, g);
                            p.push_i(threshold);
                            p.binary("ge");
                            compiled(self);
                        }
                        None => p.push_i(0),
                    },
                    None => p.push_i(0),
                }
            }
            "GetDisabled" => self.get_disabled(p, &call.on),
            "GetItemCount" => match form_arg(call, 0).and_then(|f| self.ctx.numbers.pids.get(f)) {
                Some(&pid) => {
                    let t = p.temp();
                    self.subject(p, &call.on);
                    p.store_local(t);
                    self.nonzero_or(p, t, |_, p| {
                        p.fetch_local(t);
                        p.push_i(pid as i32);
                        p.host(hw::OBJ_IS_CARRYING_OBJ_PID);
                    });
                    compiled(self);
                }
                None => {
                    self.fallback("GetItemCount of an item with no PID; read as 0");
                    p.push_i(0);
                }
            },
            "GetHasNote" => match form_arg(call, 0) {
                Some(note) => {
                    let note = SimId::of(SimKind::Note, note);
                    match self.global(&note) {
                        Some(g) => {
                            self.get_global(p, g);
                            p.push_i(0);
                            p.binary("gt");
                            compiled(self);
                        }
                        None => p.push_i(0),
                    }
                }
                None => p.push_i(0),
            },
            "GetDead" => {
                if let Some(RefExpr::Placed(r)) = &call.on
                    && let Some(g) = self.ctx.numbers.global(&derived::dead(r))
                {
                    self.get_global(p, g);
                    compiled(self);
                    return Ty::Int;
                }
                let t = p.temp();
                self.subject(p, &call.on);
                p.store_local(t);
                self.nonzero_or(p, t, |_, p| {
                    p.fetch_local(t);
                    p.host(hw::CRITTER_STATE);
                    p.push_i(1);
                    p.binary("band");
                });
                compiled(self);
            }
            "GetDeadCount" => match form_arg(call, 0).cloned() {
                Some(base) => match self.global(&derived::dead_count(&base)) {
                    Some(g) => {
                        self.get_global(p, g);
                        compiled(self);
                    }
                    None => p.push_i(0),
                },
                None => p.push_i(0),
            },
            "GetActorValue" | "GetBaseActorValue" | "GetPermanentActorValue" => {
                return self.actor_value(p, call);
            }
            "GetSecondsPassed" => {
                match self.scope.seconds {
                    Some(t) => p.fetch_local(t),
                    None => p.push_f(self.scope.seconds_const),
                }
                compiled(self);
                return Ty::Float;
            }
            "GetCurrentTime" => {
                p.host(hw::GAME_TIME_HOUR);
                p.push_i(100);
                p.binary("div");
                p.host(hw::GAME_TIME_HOUR);
                p.push_i(100);
                p.binary("mod");
                p.push_f(60.0);
                p.binary("div");
                p.binary("add");
                compiled(self);
                return Ty::Float;
            }
            "GetRandomPercent" => {
                p.push_i(0);
                p.push_i(99);
                p.host(hw::RANDOM);
                compiled(self);
            }
            "GetIsID" => {
                let form = form_arg(call, 0).cloned();
                if let (Some(base), Some(form)) = (self.on_self_base(&call.on).cloned(), &form) {
                    p.push_i(i32::from(&base == form));
                    self.count(Outcome::Folded, "GetIsID on the speaker, folded");
                } else {
                    match form
                        .as_ref()
                        .and_then(|f| self.ctx.numbers.pids.get(f))
                        .copied()
                    {
                        Some(pid) => {
                            let t = p.temp();
                            self.subject(p, &call.on);
                            p.store_local(t);
                            self.nonzero_or(p, t, |_, p| {
                                p.fetch_local(t);
                                p.host(hw::OBJ_PID);
                                p.push_i(pid as i32);
                                p.binary("eq");
                            });
                            compiled(self);
                        }
                        None => {
                            self.fallback("GetIsID of a base with no PID; read as 0");
                            p.push_i(0);
                        }
                    }
                }
            }
            "GetIsSex" | "GetPCIsSex" => {
                let female = int_arg(call, 0).unwrap_or(0);
                let on = if name == "GetPCIsSex" {
                    Some(RefExpr::Player)
                } else {
                    call.on.clone()
                };
                let folded = self
                    .on_self_base(&on)
                    .and_then(|b| self.ctx.module.actors.get(b))
                    .map(|a| a.female);
                match folded {
                    Some(is_female) if name == "GetIsSex" => {
                        p.push_i(i32::from(is_female == (female == 1)));
                        self.count(Outcome::Folded, "GetIsSex on the speaker, folded");
                    }
                    _ => {
                        self.subject(p, &on);
                        p.push_i(34);
                        p.host(hw::GET_CRITTER_STAT);
                        p.push_i(female);
                        p.binary("eq");
                        compiled(self);
                    }
                }
            }
            "GetInFaction" | "GetIsVoiceType" | "GetIsRace" | "GetIsClass" | "GetIsCreature" => {
                let actor = self
                    .on_self_base(&call.on)
                    .and_then(|b| self.ctx.module.actors.get(b));
                let form = form_arg(call, 0);
                let value = actor.map(|a| match name {
                    "GetInFaction" => form.is_some_and(|f| a.factions.iter().any(|(x, _)| x == f)),
                    "GetIsVoiceType" => form.is_some() && a.voice.as_ref() == form,
                    "GetIsRace" => form.is_some() && a.race.as_ref() == form,
                    "GetIsClass" => form.is_some() && a.class.as_ref() == form,
                    _ => a.kind == crate::ir::ActorKind::Creature,
                });
                match value {
                    Some(v) => {
                        p.push_i(i32::from(v));
                        self.count(Outcome::Folded, &format!("{name} on the speaker, folded"));
                    }
                    None => {
                        self.fallback(&format!("{name} on another actor; read as 0"));
                        p.push_i(0);
                    }
                }
            }
            "GetTalkedToPC" => {
                if self.actor.is_some() && matches!(call.on, None | Some(RefExpr::SelfRef)) {
                    self.get_local(p, locals::TALKED);
                    compiled(self);
                } else {
                    self.fallback("GetTalkedToPC outside the speaker's program; read as 0");
                    p.push_i(0);
                }
            }
            "GetIsCurrentPackage" => {
                let ordinal = self.package_ordinal(form_arg(call, 0));
                match (ordinal, self.actor.is_some()) {
                    (Some(n), true) if matches!(call.on, None | Some(RefExpr::SelfRef)) => {
                        self.get_local(p, locals::PKG);
                        p.push_i(n);
                        p.binary("eq");
                        compiled(self);
                    }
                    _ => {
                        self.fallback("GetIsCurrentPackage outside the actor's program; read as 0");
                        p.push_i(0);
                    }
                }
            }
            "GetHealthPercentage" => {
                let t = p.temp();
                self.subject(p, &call.on);
                p.store_local(t);
                let m = p.temp();
                p.fetch_local(t);
                p.push_i(7);
                p.host(hw::GET_CRITTER_STAT);
                p.store_local(m);
                self.nonzero_or(p, m, |_, p| {
                    p.push_i(100);
                    p.fetch_local(t);
                    p.push_i(35);
                    p.host(hw::GET_CRITTER_STAT);
                    p.binary("mul");
                    p.fetch_local(m);
                    p.binary("div");
                });
                compiled(self);
            }
            "GetDistance" => {
                let other = match call.args.first() {
                    Some(Expr::Ref(r)) => Some(r.clone()),
                    _ => None,
                };
                match other {
                    Some(other) => {
                        let a = p.temp();
                        let b = p.temp();
                        self.subject(p, &call.on);
                        p.store_local(a);
                        self.object(p, &other);
                        p.store_local(b);
                        // Both present: hexes times 70; else far away.
                        let far = p.label();
                        let end = p.label();
                        p.branch_false(&far, |p| p.fetch_local(a));
                        p.branch_false(&far, |p| p.fetch_local(b));
                        p.fetch_local(a);
                        p.fetch_local(b);
                        p.host(hw::TILE_DISTANCE_OBJS);
                        p.push_i(70);
                        p.binary("mul");
                        p.jump(&end);
                        p.place(&far);
                        p.push_i(1_000_000);
                        p.place(&end);
                        compiled(self);
                    }
                    None => {
                        self.fallback("GetDistance to something that is no reference");
                        p.push_i(0);
                    }
                }
            }
            "IsInCombat" => {
                p.host(hw::COMBAT_IS_INITIALIZED);
                compiled(self);
            }
            "IsInInterior" => {
                let interior = self
                    .program_map()
                    .map(|m| self.ctx.facts.interiors.contains(&m));
                match interior {
                    Some(v) => {
                        p.push_i(i32::from(v));
                        self.count(Outcome::Folded, "IsInInterior, static per map");
                    }
                    None => {
                        self.fallback("IsInInterior in a program not bound to one map; read as 0");
                        p.push_i(0);
                    }
                }
            }
            "GetIsReference" => match call.args.first() {
                Some(Expr::Ref(r)) => {
                    let r = r.clone();
                    self.subject(p, &call.on);
                    self.object(p, &r);
                    p.binary("eq");
                    compiled(self);
                }
                _ => p.push_i(0),
            },
            "GetDetected" => match call.args.first() {
                Some(Expr::Ref(r)) => {
                    let r = r.clone();
                    let a = p.temp();
                    let b = p.temp();
                    self.subject(p, &call.on);
                    p.store_local(a);
                    self.object(p, &r);
                    p.store_local(b);
                    let no = p.label();
                    let end = p.label();
                    p.branch_false(&no, |p| p.fetch_local(a));
                    p.branch_false(&no, |p| p.fetch_local(b));
                    p.fetch_local(a);
                    p.fetch_local(b);
                    p.host(hw::OBJ_CAN_SEE_OBJ);
                    p.jump(&end);
                    p.place(&no);
                    p.push_i(0);
                    p.place(&end);
                    compiled(self);
                }
                _ => p.push_i(0),
            },
            "GetLocked" => {
                let t = p.temp();
                self.subject(p, &call.on);
                p.store_local(t);
                self.nonzero_or(p, t, |_, p| {
                    p.fetch_local(t);
                    p.host(hw::OBJ_IS_LOCKED);
                });
                compiled(self);
            }
            "GetButtonPressed" => {
                self.fallback("GetButtonPressed: message boxes are not lowered; read as -1");
                p.push_i(-1);
            }
            other => {
                self.fallback(&format!("{other} has no value lowering; read as 0"));
                p.push_i(0);
            }
        }
        Ty::Int
    }

    /// Pushes `value` when the temporary `t` is not 0, else 0.
    pub fn nonzero_or(&mut self, p: &mut Proc, t: u32, value: impl FnOnce(&mut Self, &mut Proc)) {
        let zero = p.label();
        let end = p.label();
        p.branch_false(&zero, |p| p.fetch_local(t));
        value(self, p);
        p.jump(&end);
        p.place(&zero);
        p.push_i(0);
        p.place(&end);
    }

    /// The map a program runs on, when it runs on one only.
    fn program_map(&self) -> Option<String> {
        use crate::numbering::ProgramKey;
        match &self.key {
            ProgramKey::Map(m) => Some(m.clone()),
            _ => {
                let maps: std::collections::BTreeSet<&str> = self
                    .ctx
                    .facts
                    .placement
                    .objects
                    .iter()
                    .filter(|o| self.program_owns(o))
                    .map(|o| o.map.as_str())
                    .collect();
                if maps.len() == 1 {
                    maps.into_iter().next().map(str::to_owned)
                } else {
                    None
                }
            }
        }
    }

    /// Whether a placed object runs this program.
    pub fn program_owns(&self, object: &crate::ir::PlacedObject) -> bool {
        use crate::numbering::{ProgramKey, object_key};
        match &self.key {
            ProgramKey::Critter(base) => {
                object.kind == crate::ir::PlacedKind::Critter && object.base.as_ref() == Some(base)
            }
            key => object_key(self.ctx.module, self.ctx.facts, object).as_ref() == Some(key),
        }
    }

    /// The 1-based ordinal of a package in this actor's list.
    pub fn package_ordinal(&self, package: Option<&FormKey>) -> Option<i32> {
        let package = package?;
        let run = self.ctx.module.runs.get(self.actor.as_ref()?)?;
        run.slots
            .iter()
            .position(|s| &s.package.form == package)
            .map(|i| i as i32 + 1)
    }

    /// `GetDisabled` (spec 8): the group's state, or a reported 0.
    fn get_disabled(&mut self, p: &mut Proc, on: &Option<RefExpr>) {
        let form = match on {
            Some(RefExpr::Placed(form)) => Some(form.clone()),
            _ => None,
        };
        if let Some(form) = &form {
            if let Some(group) = self.group_of(form)
                && let Some(g) = self.global(&group)
            {
                self.get_global(p, g);
                p.push_i(0);
                p.binary("eq");
                self.count(Outcome::Compiled, "");
                return;
            }
            if let Some((group, opposite)) = self
                .ctx
                .groups_of_child
                .get(form)
                .and_then(|v| v.first())
                .cloned()
                && let Some(g) = self.global(&group)
            {
                self.get_global(p, g);
                p.push_i(0);
                p.binary(if opposite { "ne" } else { "eq" });
                self.count(Outcome::Compiled, "");
                return;
            }
        }
        self.fallback("GetDisabled of a reference in no group; read as 0");
        p.push_i(0);
    }

    /// A call whose value is not used.
    pub fn call_stmt(&mut self, p: &mut Proc, call: &Call) {
        let name = match &call.func {
            Func::Known(id) => id.name(),
            Func::Unsupported { name } => {
                let name = name.clone();
                self.count(Outcome::Dropped, &format!("{name}: not supported"));
                return;
            }
        };
        let compiled = |g: &mut Self| g.count(Outcome::Compiled, "");
        match name {
            "SetStage" => match quest_arg(call).cloned() {
                Some(q) => {
                    let stage = call.args.get(1).cloned().unwrap_or(Expr::Int(0));
                    self.set_stage(p, &q, &stage);
                    compiled(self);
                }
                None => self.fallback("SetStage of no quest; dropped"),
            },
            "StartQuest" | "StopQuest" => match quest_arg(call).cloned() {
                Some(q) => {
                    if let Some(g) = self.quest_global(&q, quest_sub::FLAGS) {
                        if name == "StartQuest" {
                            self.set_bits(p, g, 1);
                        } else {
                            self.clear_bits(p, g, 1);
                        }
                        compiled(self);
                    }
                }
                None => self.fallback(&format!("{name} of no quest; dropped")),
            },
            "SetObjectiveDisplayed" | "SetObjectiveCompleted" => {
                let Some(q) = quest_arg(call).cloned() else {
                    self.fallback(&format!("{name} of no quest; dropped"));
                    return;
                };
                let Some(g) = self.objective_global(&q, call) else {
                    return;
                };
                let flag = call.args.get(2).cloned().unwrap_or(Expr::Int(1));
                let (on_value, off_value) = if name == "SetObjectiveDisplayed" {
                    (1, 0)
                } else {
                    (2, 1)
                };
                let off = p.label();
                let end = p.label();
                self.expr_false(p, &flag, &off);
                if on_value == 1 {
                    // Displayed: 0 becomes 1; a completed objective stays.
                    let skip = p.label();
                    p.branch_false(&skip, |p| {
                        p.push_i(g as i32);
                        p.host(hw::GLOBAL_VAR);
                        p.push_i(1);
                        p.binary("lt");
                    });
                    self.set_global(p, g, 1);
                    p.place(&skip);
                } else {
                    self.set_global(p, g, 2);
                }
                p.jump(&end);
                p.place(&off);
                if on_value == 1 {
                    self.set_global(p, g, off_value);
                } else {
                    let skip = p.label();
                    p.branch_false(&skip, |p| {
                        p.push_i(g as i32);
                        p.host(hw::GLOBAL_VAR);
                        p.push_i(2);
                        p.binary("eq");
                    });
                    self.set_global(p, g, off_value);
                    p.place(&skip);
                }
                p.place(&end);
                self.count_in(Section::Quests, Outcome::Compiled, "");
                compiled(self);
            }
            "Enable" | "Disable" => {
                self.enable(p, &call.on, name == "Enable");
                compiled(self);
            }
            "AddItem" => match form_arg(call, 0)
                .and_then(|f| self.ctx.numbers.pids.get(f))
                .copied()
            {
                Some(pid) => {
                    let count = call.args.get(1).cloned().unwrap_or(Expr::Int(1));
                    self.with_object(p, &call.on, |g, p, t| {
                        p.fetch_local(t);
                        p.push_i(pid as i32);
                        p.push_i(0);
                        p.push_i(0);
                        p.push_i(-1);
                        p.host(hw::CREATE_OBJECT_SID);
                        g.expr_int(p, &count);
                        p.host(hw::ADD_MULT_OBJS_TO_INVEN);
                    });
                    compiled(self);
                }
                None => self.fallback("AddItem of an item with no PID; dropped"),
            },
            "RemoveItem" => match form_arg(call, 0)
                .and_then(|f| self.ctx.numbers.pids.get(f))
                .copied()
            {
                Some(pid) => {
                    let count = call.args.get(1).cloned().unwrap_or(Expr::Int(1));
                    self.with_object(p, &call.on, |g, p, t| {
                        let item = p.temp();
                        p.fetch_local(t);
                        p.push_i(pid as i32);
                        p.host(hw::OBJ_CARRYING_PID_OBJ);
                        p.store_local(item);
                        let skip = p.label();
                        p.branch_false(&skip, |p| p.fetch_local(item));
                        p.fetch_local(t);
                        p.fetch_local(item);
                        g.expr_int(p, &count);
                        p.host_stmt(hw::RM_MULT_OBJS_FROM_INVEN);
                        p.fetch_local(item);
                        p.host(hw::DESTROY_OBJECT);
                        p.place(&skip);
                    });
                    compiled(self);
                }
                None => self.fallback("RemoveItem of an item with no PID; dropped"),
            },
            "RemoveAllItems" => {
                let target = match call.args.first() {
                    Some(Expr::Ref(r)) => Some(r.clone()),
                    _ => None,
                };
                match target {
                    Some(target) => {
                        let d = p.temp();
                        self.object(p, &target);
                        p.store_local(d);
                        self.with_object(p, &call.on, |_, p, t| {
                            let skip = p.label();
                            p.branch_false(&skip, |p| p.fetch_local(d));
                            p.fetch_local(t);
                            p.fetch_local(d);
                            p.host(hw::MOVE_OBJ_INVEN_TO_OBJ);
                            p.place(&skip);
                        });
                        compiled(self);
                    }
                    None => self.fallback(
                        "RemoveAllItems without a destination: items are not destroyed; dropped",
                    ),
                }
            }
            "AddNote" | "RemoveNote" => match form_arg(call, 0).cloned() {
                Some(note) => {
                    let note = SimId::of(SimKind::Note, &note);
                    if name == "AddNote" {
                        self.add_note(p, &note);
                    } else if let Some(g) = self.global(&note) {
                        self.set_global(p, g, 0);
                    }
                    self.count_in(Section::Terminals, Outcome::Compiled, "");
                    compiled(self);
                }
                None => self.fallback(&format!("{name} of no note; dropped")),
            },
            "AddTopic" => match form_arg(call, 0).cloned() {
                Some(topic) => {
                    if let Some(g) = self.global(&derived::topic_given(&topic)) {
                        self.set_global(p, g, 1);
                        compiled(self);
                    }
                }
                None => self.fallback("AddTopic of no topic; dropped"),
            },
            "MoveTo" => {
                let target = match call.args.first() {
                    Some(Expr::Ref(r)) => Some(r.clone()),
                    _ => None,
                };
                let Some(target) = target else {
                    self.fallback("MoveTo of no reference; dropped");
                    return;
                };
                let d = p.temp();
                self.object(p, &target);
                p.store_local(d);
                self.with_object(p, &call.on, |_, p, t| {
                    let skip = p.label();
                    p.branch_false(&skip, |p| p.fetch_local(d));
                    p.fetch_local(t);
                    p.fetch_local(d);
                    p.host(hw::TILE_NUM);
                    p.fetch_local(d);
                    p.host(hw::ELEVATION);
                    p.host_stmt(hw::MOVE_TO);
                    p.place(&skip);
                });
                self.count(
                    Outcome::Fallback,
                    "MoveTo: same map only; a target on another map is skipped (gap M)",
                );
            }
            "Kill" => {
                self.with_object(p, &call.on, |_, p, t| {
                    p.fetch_local(t);
                    p.push_i(0);
                    p.host(hw::KILL_CRITTER);
                });
                compiled(self);
            }
            "EvaluatePackage" | "ResetAI" | "AddScriptPackage" | "RemoveScriptPackage" => {
                let on_self = matches!(call.on, None | Some(RefExpr::SelfRef));
                if self.actor.is_none() || !on_self {
                    self.fallback(&format!(
                        "{name} on another actor: runner state is per program; dropped"
                    ));
                    return;
                }
                match name {
                    "AddScriptPackage" => {
                        match self.package_ordinal(form_arg(call, 0)) {
                            Some(n) => self.set_local(p, locals::SCRIPTPKG, n),
                            None => {
                                self.fallback("AddScriptPackage of a package not in the actor's list; dropped");
                                return;
                            }
                        }
                    }
                    "RemoveScriptPackage" => self.set_local(p, locals::SCRIPTPKG, 0),
                    _ => {}
                }
                self.set_local(p, locals::EVP, 1);
                self.count_in(Section::Packages, Outcome::Compiled, "");
                compiled(self);
            }
            "StartCombat" => {
                let target = match call.args.first() {
                    Some(Expr::Ref(r)) => r.clone(),
                    _ => RefExpr::Player,
                };
                let d = p.temp();
                self.object(p, &target);
                p.store_local(d);
                self.with_object(p, &call.on, |_, p, t| {
                    let skip = p.label();
                    p.branch_false(&skip, |p| p.fetch_local(d));
                    p.fetch_local(t);
                    p.fetch_local(d);
                    p.host(hw::ATTACK_SETUP);
                    p.place(&skip);
                });
                compiled(self);
            }
            "SetActorValue" | "ModActorValue" | "RestoreActorValue" => {
                let health = matches!(call.args.first(), Some(Expr::Int(16)));
                if !health {
                    self.fallback(&format!(
                        "{name} of an actor value other than Health; dropped"
                    ));
                    return;
                }
                let amount = call.args.get(1).cloned().unwrap_or(Expr::Int(0));
                let set = name == "SetActorValue";
                self.with_object(p, &call.on, |g, p, t| {
                    p.fetch_local(t);
                    g.expr_int(p, &amount);
                    if set {
                        p.fetch_local(t);
                        p.push_i(35);
                        p.host(hw::GET_CRITTER_STAT);
                        p.binary("sub");
                    }
                    p.host_stmt(hw::CRITTER_HEAL);
                });
                compiled(self);
            }
            "RewardXP" => {
                let amount = call.args.first().cloned().unwrap_or(Expr::Int(0));
                self.expr_int(p, &amount);
                p.host(hw::GIVE_EXP_POINTS);
                compiled(self);
            }
            "RewardKarma" => match self.ctx.numbers.global(&derived::karma()) {
                Some(g) => {
                    let amount = call.args.first().cloned().unwrap_or(Expr::Int(0));
                    self.set_global_with(p, g, |gw, p| {
                        gw.get_global(p, g);
                        gw.expr_int(p, &amount);
                        p.binary("add");
                    });
                    self.count(
                        Outcome::Fallback,
                        "RewardKarma: shadow global, not shown (gap K)",
                    );
                }
                None => self.fallback("RewardKarma without the karma global; dropped"),
            },
            "ShowBarterMenu" => {
                if self.scope.dialogue {
                    p.push_i(0);
                    p.host(hw::GDIALOG_BARTER);
                    compiled(self);
                } else {
                    self.fallback("ShowBarterMenu outside dialogue; dropped");
                }
            }
            "Lock" | "Unlock" => {
                let word = if name == "Lock" {
                    hw::OBJ_LOCK
                } else {
                    hw::OBJ_UNLOCK
                };
                self.with_object(p, &call.on, |_, p, t| {
                    p.fetch_local(t);
                    p.host(word);
                });
                self.count_in(Section::Terminals, Outcome::Compiled, "");
                compiled(self);
            }
            "SetOpenState" => {
                let state = call.args.first().cloned().unwrap_or(Expr::Int(1));
                self.with_object(p, &call.on, |g, p, t| {
                    let close = p.label();
                    let end = p.label();
                    g.expr_false(p, &state, &close);
                    p.fetch_local(t);
                    p.host(hw::OBJ_OPEN);
                    p.jump(&end);
                    p.place(&close);
                    p.fetch_local(t);
                    p.host(hw::OBJ_CLOSE);
                    p.place(&end);
                });
                compiled(self);
            }
            "Activate" => {
                let on_self = matches!(call.on, None | Some(RefExpr::SelfRef));
                if on_self {
                    match self.scope.activation {
                        Some(t) => {
                            p.push_i(1);
                            p.store_local(t);
                            compiled(self);
                        }
                        None => self.count(
                            Outcome::Folded,
                            "Activate on self outside an activation: the default action runs",
                        ),
                    }
                } else {
                    self.with_object(p, &call.on, |_, p, t| {
                        p.fetch_local(t);
                        p.host(hw::USE_OBJ);
                    });
                    compiled(self);
                }
            }
            "DisablePlayerControls" => {
                p.host(hw::GAME_UI_DISABLE);
                compiled(self);
            }
            "EnablePlayerControls" => {
                p.host(hw::GAME_UI_ENABLE);
                compiled(self);
            }
            "Say" | "SayTo" => self.say_float(p, call),
            "ShowMessage" => {
                let message = form_arg(call, 0)
                    .and_then(|f| {
                        self.ctx
                            .module
                            .messages
                            .get(&SimId::of(SimKind::Message, f))
                    })
                    .cloned();
                match message.and_then(|m| m.text.map(|t| (m.message_box, t))) {
                    Some((boxed, text)) => {
                        let text = self.text(text);
                        let id = self.msg(&text);
                        self.display(p, None, id);
                        if boxed {
                            self.count(
                                Outcome::Fallback,
                                "ShowMessage of a message box: shown as a notice, buttons not lowered",
                            );
                        } else {
                            compiled(self);
                        }
                    }
                    None => {
                        self.fallback("ShowMessage of a message the module does not hold; dropped")
                    }
                }
            }
            "PlaceLeveledActorAtMe" | "AddToFaction" | "RemoveFromFaction" | "SetFactionRank" => {
                self.fallback(&format!(
                    "{name}: the IR does not carry what it needs; dropped"
                ));
            }
            other => {
                // A value function used as a statement: evaluate nothing.
                self.count(
                    Outcome::Dropped,
                    &format!("{other} as a statement has no effect"),
                );
            }
        }
    }

    /// `Say`/`SayTo`: a floating line of the topic's first INFO for this
    /// speaker (spec 5.9), in the speaker's own program only.
    fn say_float(&mut self, p: &mut Proc, call: &Call) {
        let topic = match (call.func.clone(), call.args.as_slice()) {
            (_, [Expr::Form(t), ..]) => Some(t.clone()),
            (_, [_, Expr::Form(t), ..]) => Some(t.clone()),
            _ => None,
        };
        let on_self = matches!(call.on, None | Some(RefExpr::SelfRef));
        let line = match (&topic, on_self) {
            (Some(topic), true) => self.first_line(&SimId::of(SimKind::Topic, topic)),
            _ => None,
        };
        match line {
            Some(id) => {
                self.self_obj(p);
                self.message_str(p, id);
                p.push_i(0);
                p.host(hw::FLOAT_MSG);
                self.count(
                    Outcome::Fallback,
                    "Say: the topic's first line as a floating message",
                );
            }
            None => self.fallback("Say of a topic this program cannot say; dropped"),
        }
    }

    /// `AddNote` (spec 9.2): the note's global, and a message-line notice.
    pub fn add_note(&mut self, p: &mut Proc, note: &SimId) {
        let Some(g) = self.global(note) else {
            return;
        };
        self.set_global(p, g, 1);
        let title = self
            .ctx
            .module
            .notes
            .get(note)
            .and_then(|n| n.title)
            .map(|t| self.text(t));
        if let Some(title) = title {
            let id = self.msg(&title);
            self.display(p, Some(14), id);
        }
    }

    /// Compares a stat read of the player with a value (spec 4.7).
    pub fn player_stat(&mut self, p: &mut Proc, stat: Stat) {
        p.host(hw::DUDE_OBJ);
        match stat {
            Stat::Skill(skill) => {
                p.push_i(skill as i32);
                p.host(hw::HAS_SKILL);
            }
            Stat::Special(n) => {
                p.push_i(i32::from(n));
                p.host(hw::GET_CRITTER_STAT);
            }
        }
    }

    /// Jumps to `label` unless `stat op value` holds for the player.
    pub fn stat_false(&mut self, p: &mut Proc, stat: Stat, op: CmpOp, value: i32, label: &str) {
        p.branch_false(label, |p| {
            self.player_stat(p, stat);
            p.push_i(value);
            p.binary(super::generator::cmp_word(op));
        });
    }
}
