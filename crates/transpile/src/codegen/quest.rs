//! The quest dispatcher `mfquest` (spec 5.6, 6.2): a manifest global
//! script whose `start` runs every few steps, polls the running quests'
//! `GameMode` bodies by game time, and runs pending stage scripts; its hook
//! procedure `mode_changed` runs pending stage scripts when the game mode
//! changes (dialogue closing).

use super::asm::Proc;
use super::generator::{Gen, Section};
use super::words::{self as hw};
use crate::ir::{Event, Quest, SimId, quest_sub};

/// The most due times the dispatcher keeps (one program variable each).
pub const MAX_DUE_BUCKETS: usize = 64;
use crate::lowered::Outcome;

impl Gen<'_, '_> {
    fn quest_body(&self, quest: &Quest) -> Vec<Vec<crate::ir::Stmt>> {
        let Some(script) = quest
            .script
            .as_ref()
            .and_then(|s| self.ctx.module.scripts.get(s))
        else {
            return Vec::new();
        };
        script
            .blocks
            .iter()
            .filter(|b| b.event == Event::GameMode)
            .map(|b| b.body.clone())
            .collect()
    }

    /// The dispatcher's procedures.
    pub fn dispatcher(&mut self) {
        self.scope.has_self = false;
        self.scope.section = Some(Section::Quests);
        let module = self.ctx.module;
        let quests: Vec<&Quest> = module.quests.values().collect();
        let polled: Vec<(&Quest, Vec<Vec<crate::ir::Stmt>>)> = quests
            .iter()
            .map(|q| (*q, self.quest_body(q)))
            .filter(|(_, b)| !b.is_empty())
            .collect();
        // One due time per bucket of quests (spec 5.6): quest i polls in
        // bucket i % B, with B at most MAX_DUE_BUCKETS, so the program
        // variables (on the value stack) stay bounded however many quests
        // the module has. With no more quests than buckets every quest has
        // its own due time, staggered by 3 ticks.
        let buckets = polled.len().min(MAX_DUE_BUCKETS);
        let inited = self.asm.pvar("q_init", 0);
        let dues: Vec<u32> = (0..buckets)
            .map(|i| self.asm.pvar(&format!("q_due{}", i + 1), 0))
            .collect();
        let delay_of = |quest: &Quest| {
            quest
                .delay_seconds
                .filter(|d| *d > 0.0)
                .unwrap_or(crate::lowered::QUEST_DELAY_SECONDS)
        };

        let mut p = Proc::new("start");
        self.scope.temps.clear();
        let ready = p.label();
        p.branch_false(&ready, |p| {
            p.fetch_global(inited);
            p.push_i(0);
            p.binary("eq");
        });
        for (i, due) in dues.iter().enumerate() {
            p.host(hw::GAME_TIME);
            p.push_i(3 * i as i32);
            p.binary("add");
            p.store_global(*due);
        }
        p.push_i(1);
        p.store_global(inited);
        p.place(&ready);
        for (b, due) in dues.iter().enumerate() {
            let members: Vec<&(&Quest, Vec<Vec<crate::ir::Stmt>>)> =
                polled.iter().skip(b).step_by(buckets).collect();
            let delay = members
                .iter()
                .map(|(q, _)| delay_of(q))
                .fold(f32::INFINITY, f32::min);
            if members.len() > 1 && members.iter().any(|(q, _)| delay_of(q) != delay) {
                self.count_in(
                    Section::Quests,
                    Outcome::Fallback,
                    "quest polled with the shortest delay of its dispatcher bucket",
                );
            }
            // The bucket is a procedure of its own: `start` stays short.
            let name = format!("q_poll{}", b + 1);
            let mut poll = Proc::new(&name);
            let skip = poll.label();
            poll.branch_false(&skip, |p| {
                p.host(hw::GAME_TIME);
                p.fetch_global(*due);
                p.binary("ge");
            });
            poll.host(hw::GAME_TIME);
            poll.push_i((delay * 10.0).round().max(1.0) as i32);
            poll.binary("add");
            poll.store_global(*due);
            for (quest, bodies) in members {
                let mark = poll.mark();
                self.scope.owner = quest.editor_id.clone();
                self.scope.seconds = None;
                self.scope.seconds_const = delay_of(quest);
                let Some(flags) = self
                    .ctx
                    .numbers
                    .global(&SimId::quest_state(&quest.id.form, quest_sub::FLAGS))
                else {
                    continue;
                };
                let next = poll.label();
                poll.branch_false(&next, |p| {
                    p.push_i(flags as i32);
                    p.host(hw::GLOBAL_VAR);
                    p.push_i(1);
                    p.binary("band");
                });
                self.scope.section = Some(Section::Scripts);
                for body in bodies {
                    self.block(&mut poll, body);
                }
                self.scope.section = Some(Section::Quests);
                poll.place(&next);
                poll.release(mark);
                self.count_in(Section::Quests, Outcome::Compiled, "");
            }
            poll.place(&skip);
            self.polls.push(poll);
            p.call_stmt(&name);
        }
        p.call_stmt("pend");
        self.asm.add(p);
        for poll in std::mem::take(&mut self.polls) {
            self.asm.add(poll);
        }

        let mut p = Proc::new("pend");
        self.scope.temps.clear();
        for quest in quests {
            self.pending_stages(&mut p, quest);
        }
        self.asm.add(p);

        let mut p = Proc::new("mode_changed");
        p.call_stmt("pend");
        self.asm.add(p);
    }

    /// Runs the pending stage scripts of one quest, in stage order
    /// (spec 6.2): for each pending stage, every log entry in file order
    /// whose condition passes runs (its script, complete and fail flags,
    /// next quest). Each condition is tested when its entry is reached, so
    /// an earlier entry's script can change whether a later entry passes,
    /// as the source runs them one after another.
    fn pending_stages(&mut self, p: &mut Proc, quest: &Quest) {
        self.scope.owner = quest.editor_id.clone();
        let q = quest.id.form.clone();
        let mut stages: Vec<&crate::ir::Stage> = quest.stages.iter().collect();
        stages.sort_by_key(|s| s.index);
        for stage in stages {
            if stage.entries.is_empty() {
                continue;
            }
            let Some((word, bit)) = self.stage_bit(&q, i32::from(stage.index)) else {
                continue;
            };
            let Some(pend) = self
                .ctx
                .numbers
                .global(&SimId::quest_state(&q, quest_sub::PEND + word))
            else {
                continue;
            };
            let skip = p.label();
            p.branch_false(&skip, |p| {
                p.push_i(pend as i32);
                p.host(hw::GLOBAL_VAR);
                p.push_i(bit);
                p.binary("band");
            });
            self.clear_bits(p, pend, bit);
            for entry in &stage.entries {
                let mark = p.mark();
                let next = p.label();
                self.scope.section = Some(Section::Quests);
                self.cond_false(p, &entry.condition, &next);
                if let Some(body) = entry
                    .script
                    .as_ref()
                    .and_then(|s| self.ctx.module.scripts.get(s))
                    .and_then(|s| s.blocks.first())
                    .map(|b| b.body.clone())
                {
                    self.scope.section = Some(Section::Scripts);
                    self.block(p, &body);
                    self.scope.section = Some(Section::Quests);
                }
                let flags = self
                    .ctx
                    .numbers
                    .global(&SimId::quest_state(&q, quest_sub::FLAGS));
                if entry.complete
                    && let Some(flags) = flags
                {
                    self.set_bits(p, flags, 2);
                    self.clear_bits(p, flags, 1);
                    for objective in &quest.objectives {
                        if let Some(g) = self.ctx.numbers.global(&objective.id) {
                            let keep = p.label();
                            p.branch_false(&keep, |p| {
                                p.push_i(g as i32);
                                p.host(hw::GLOBAL_VAR);
                                p.push_i(1);
                                p.binary("eq");
                            });
                            self.set_global(p, g, 2);
                            p.place(&keep);
                        }
                    }
                }
                if entry.fail
                    && let Some(flags) = flags
                {
                    self.set_bits(p, flags, 4);
                    self.clear_bits(p, flags, 1);
                }
                if let Some(next_quest) = &entry.next_quest
                    && let Some(g) = self
                        .ctx
                        .numbers
                        .global(&SimId::quest_state(&next_quest.form, quest_sub::FLAGS))
                {
                    self.set_bits(p, g, 1);
                }
                p.place(&next);
                p.release(mark);
                self.count_in(Section::Quests, Outcome::Compiled, "");
            }
            p.place(&skip);
        }
    }
}
