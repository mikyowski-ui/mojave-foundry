//! Dialogue lowering, per speaker (spec 4.2 to 4.7, 4.10): the reply and
//! option machine of SPEC-006 section 3.9 with numeric callbacks.
//!
//! Procedures (spec 4.4):
//!
//! - `p_<t>`: the pick of topic t, an INFO ordinal or 0: order, random
//!   groups (one `random(1, n)` over the passing members), say-once and
//!   say-once-a-day. No side effect but the draw.
//! - `i_<n>`: say INFO n: flags, the talked-to local, its begin block, its
//!   end block pending (or at once with run-immediately), the reply, then
//!   its options: Done for goodbye, the same list for a refusal, its
//!   choices, or the hub. `cn_<n>_<r>`: the continuation parts of a reply
//!   longer than one message.
//! - `ch_<n>`: a Fallout 3 percentage challenge: the stored outcome or one
//!   roll, then the INFO (with the speech experience) or the failure route.
//! - `l_<n>`, `hub`: option lists. Each shown option stores the INFO its
//!   pick chose in `d_k<slot>`, so the option said is the option shown.
//! - `o_<slot>`: option callbacks: run the pending end block, then `say`
//!   the slot's INFO. `say` and `run_end` dispatch on an ordinal; `x_end`
//!   runs the end block and builds nothing (the loop ends); `relist`
//!   rebuilds the current list.
//!
//! Message ids (spec 4.10): labels 1 to 99; `10000 + 10 n + r` reply part
//! r of INFO n; `10000 + 10 n + 9` its option text (the prompt, else the
//! topic's name).

use std::collections::BTreeMap;

use super::asm::Proc;
use super::generator::{Gen, Section, locals};
use super::words::{self as hw};
use crate::ir::{CmpOp, Cond, FailureRoute, Pick, Remember, SimId, SkillCheck, Stat};
use crate::lowered::Outcome;
use crate::numbering::derived;

/// The most bytes one reply message holds (SPEC-006 section 3.10).
pub const REPLY_BYTES: usize = 899;
/// The most options of one list: 30, less the Goodbye or Done option.
pub const MAX_LIST: usize = 29;

#[derive(Debug, Clone)]
pub(crate) struct TopicPlan {
    pub id: SimId,
    /// The picks, by INFO index (ordinal - 1).
    pub picks: Vec<Vec<usize>>,
}

#[derive(Debug, Clone)]
pub(crate) struct InfoPlan {
    pub id: SimId,
    pub cond: Cond,
    pub parts: Vec<String>,
    pub choices: Vec<usize>,
    pub prompt: Option<crate::ir::TextId>,
    pub failure: Option<usize>,
    pub topic: usize,
}

/// One speaker's dialogue, from `Module::dialogues` (task T5's
/// [`crate::ir::Dialogue`]) with topics and INFOs as indices.
#[derive(Debug, Clone, Default)]
pub(crate) struct DialoguePlan {
    pub topics: Vec<TopicPlan>,
    pub infos: Vec<InfoPlan>,
    pub greeting: Option<usize>,
    /// Hub topics, with the "given" global a non-top-level one needs.
    pub hub: Vec<(usize, Option<u32>)>,
    pub slots: usize,
}

impl DialoguePlan {
    fn ordinal(index: usize) -> i32 {
        index as i32 + 1
    }

    /// The reply part id of INFO `index`.
    pub fn part_id(index: usize, part: usize) -> i32 {
        10_000 + 10 * Self::ordinal(index) + part as i32
    }

    /// The option text id of INFO `index`.
    pub fn option_id(index: usize) -> i32 {
        10_000 + 10 * Self::ordinal(index) + 9
    }

    pub fn is_empty(&self) -> bool {
        self.infos.is_empty()
    }
}

/// Splits a reply into message-sized parts (at most 9) at spaces.
fn split_reply(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && parts.len() < 9 {
        if rest.len() <= REPLY_BYTES {
            parts.push(rest.to_owned());
            break;
        }
        let mut cut = REPLY_BYTES;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let at = rest[..cut].rfind(' ').filter(|&i| i > 0).unwrap_or(cut);
        parts.push(rest[..at].trim_end().to_owned());
        rest = rest[at..].trim_start();
    }
    if parts.is_empty() {
        parts.push(String::new());
    }
    parts
}

impl Gen<'_, '_> {
    /// The speaker's plan from `Module::dialogues`, counted.
    pub fn plan_dialogue(&mut self, base: &crate::ir::FormKey) -> DialoguePlan {
        let module = self.ctx.module;
        let Some(dialogue) = module.dialogues.get(base) else {
            return DialoguePlan::default();
        };
        self.scope.owner = dialogue.editor_id.clone();
        let c = dialogue.counts;
        for (outcome, reason, n) in [
            (
                Outcome::Folded,
                "INFO folded away for this speaker",
                c.folded_away,
            ),
            (
                Outcome::Dropped,
                "bark INFO not lowered (owner decision 14)",
                c.barks,
            ),
            (
                Outcome::Dropped,
                "INFO of a topic nothing leads to",
                c.unreached,
            ),
        ] {
            for _ in 0..n {
                self.count_in(Section::Dialogue, outcome, reason);
            }
        }
        let topic_index: BTreeMap<SimId, usize> = dialogue
            .topics
            .keys()
            .enumerate()
            .map(|(i, t)| (t.clone(), i))
            .collect();
        let mut plan = DialoguePlan::default();
        for topic in dialogue.topics.values() {
            let picks = topic
                .picks
                .iter()
                .map(|pick| match pick {
                    Pick::Info(n) => vec![*n as usize - 1],
                    Pick::Random(group) => group.iter().map(|n| *n as usize - 1).collect(),
                })
                .collect();
            plan.topics.push(TopicPlan {
                id: topic.topic.clone(),
                picks,
            });
        }
        for node in &dialogue.nodes {
            self.count_in(Section::Dialogue, Outcome::Compiled, "");
            let info = module.infos.get(&node.info);
            let text: Vec<String> = info
                .map(|info| {
                    let mut responses = info.responses.clone();
                    responses.sort_by_key(|r| r.number);
                    responses.iter().map(|r| self.text(r.text)).collect()
                })
                .unwrap_or_default();
            let joined = text.join(" ");
            if joined.trim().len() > REPLY_BYTES * 9 {
                self.count_in(
                    Section::Dialogue,
                    Outcome::Fallback,
                    "reply longer than nine messages; cut",
                );
            }
            let mut choices: Vec<usize> = node
                .choices
                .iter()
                .filter_map(|t| topic_index.get(t).copied())
                .collect();
            if choices.len() > MAX_LIST {
                self.count_in(
                    Section::Dialogue,
                    Outcome::Fallback,
                    "more than 29 choices; cut",
                );
                choices.truncate(MAX_LIST);
            }
            plan.infos.push(InfoPlan {
                id: node.info.clone(),
                cond: node.condition.clone(),
                parts: split_reply(&joined),
                choices,
                prompt: node.prompt,
                failure: node
                    .failure
                    .as_ref()
                    .and_then(|t| topic_index.get(t).copied()),
                topic: topic_index.get(&node.topic).copied().unwrap_or(0),
            });
        }
        plan.greeting = dialogue
            .greeting
            .as_ref()
            .and_then(|g| topic_index.get(g).copied());
        if dialogue.greeting_label || plan.greeting.is_none() {
            self.count_in(
                Section::Dialogue,
                Outcome::Fallback,
                "no greeting INFO for this speaker: a converter greeting opens the hub",
            );
        }
        for entry in &dialogue.hub {
            let Some(&t) = topic_index.get(&entry.topic) else {
                continue;
            };
            let given = entry
                .given
                .as_ref()
                .and_then(|g| self.ctx.numbers.global(g));
            if entry.given.is_some() && given.is_none() {
                continue;
            }
            plan.hub.push((t, given));
        }
        if plan.hub.len() > MAX_LIST {
            self.count_in(
                Section::Dialogue,
                Outcome::Fallback,
                "more than 29 hub topics; cut",
            );
            plan.hub.truncate(MAX_LIST);
        }
        plan.slots = plan
            .infos
            .iter()
            .map(|i| i.choices.len())
            .chain([plan.hub.len()])
            .max()
            .unwrap_or(0)
            .max(1);
        plan
    }

    /// The program messages of the plan.
    fn dialogue_messages(&mut self, plan: &DialoguePlan) {
        let module = self.ctx.module;
        for (n, info) in plan.infos.iter().enumerate() {
            for (r, part) in info.parts.iter().enumerate() {
                self.put_msg(DialoguePlan::part_id(n, r), part.clone());
            }
            let option = match info.prompt {
                Some(t) => self.text(t),
                None => module
                    .topics
                    .get(&plan.topics[info.topic].id)
                    .map(|t| t.editor_id.clone())
                    .unwrap_or_default(),
            };
            self.put_msg(DialoguePlan::option_id(n), option);
        }
    }

    /// The message id of the first line the topic says, for `Say`.
    pub fn first_line(&self, topic: &SimId) -> Option<i32> {
        let plan = self.dialogue.as_ref()?;
        let t = plan.topics.iter().position(|x| &x.id == topic)?;
        plan.topics[t]
            .picks
            .first()
            .and_then(|p| p.first())
            .map(|&n| DialoguePlan::part_id(n, 0))
    }

    /// Jumps to `label` unless INFO `n` can be said now.
    fn passes_false(&mut self, p: &mut Proc, plan: &DialoguePlan, n: usize, label: &str) {
        let info = &self.ctx.module.infos[&plan.infos[n].id];
        let flags = info.flags;
        let id = info.id.clone();
        if flags.say_once
            && let Some(g) = self.ctx.numbers.global(&derived::say_once(&id))
        {
            p.branch_false(label, |p| {
                p.push_i(g as i32);
                p.host(hw::GLOBAL_VAR);
                p.push_i(0);
                p.binary("eq");
            });
        }
        if flags.say_once_a_day
            && let Some(g) = self.ctx.numbers.global(&derived::say_day(&id))
        {
            p.branch_false(label, |p| {
                p.push_i(g as i32);
                p.host(hw::GLOBAL_VAR);
                p.host(hw::GAME_TIME);
                p.push_i(864_000);
                p.binary("div");
                p.binary("ne");
            });
        }
        let cond = plan.infos[n].cond.clone();
        self.cond_false(p, &cond, label);
    }

    fn pick(&mut self, plan: &DialoguePlan, t: usize) {
        let mut p = Proc::with_result(&format!("p_{}", t + 1));
        let result = p.result_slot().unwrap_or(0);
        self.scope.temps.clear();
        for group in plan.topics[t].picks.clone() {
            let mark = p.mark();
            if let [n] = group.as_slice() {
                let skip = p.label();
                self.passes_false(&mut p, plan, *n, &skip);
                p.push_i(DialoguePlan::ordinal(*n));
                p.store_local(result);
                p.ret();
                p.place(&skip);
                p.release(mark);
                continue;
            }
            let count = p.temp();
            for &m in &group {
                let skip = p.label();
                self.passes_false(&mut p, plan, m, &skip);
                p.fetch_local(count);
                p.push_i(1);
                p.binary("add");
                p.store_local(count);
                p.place(&skip);
            }
            let none = p.label();
            p.branch_false(&none, |p| p.fetch_local(count));
            let draw = p.temp();
            p.push_i(1);
            p.fetch_local(count);
            p.host(hw::RANDOM);
            p.store_local(draw);
            for &m in &group {
                let skip = p.label();
                self.passes_false(&mut p, plan, m, &skip);
                p.fetch_local(draw);
                p.push_i(1);
                p.binary("sub");
                p.store_local(draw);
                p.branch_false(&skip, |p| {
                    p.fetch_local(draw);
                    p.push_i(0);
                    p.binary("eq");
                });
                p.push_i(DialoguePlan::ordinal(m));
                p.store_local(result);
                p.ret();
                p.place(&skip);
            }
            p.place(&none);
            p.release(mark);
        }
        self.asm.add(p);
    }

    /// The Fallout 3 chance, as `dialogue::checks::fo3_chance` computes it
    /// (spec 4.7, calibration C-2): pushes `clamp(round(S * M) - D, 0,
    /// 100)`, or 100 when that is at least `100 *` the auto-success
    /// fraction.
    fn chance(&mut self, p: &mut Proc, difficulty: u8) {
        let settings = &self.ctx.settings;
        let m = settings.speech_multiplier;
        let d = settings.speech_difficulty[usize::from(difficulty.clamp(1, 5) - 1)];
        let auto = f64::from(settings.speech_auto_success) * 100.0;
        let c = p.temp();
        p.host(hw::DUDE_OBJ);
        p.push_i(14);
        p.host(hw::HAS_SKILL);
        p.push_f(m);
        p.binary("mul");
        self.round(p);
        p.push_i(d);
        p.binary("sub");
        p.store_local(c);
        let low = p.label();
        p.branch_false(&low, |p| {
            p.fetch_local(c);
            p.push_i(0);
            p.binary("lt");
        });
        p.push_i(0);
        p.store_local(c);
        p.place(&low);
        let high = p.label();
        p.branch_false(&high, |p| {
            p.fetch_local(c);
            p.push_i(100);
            p.binary("gt");
        });
        p.push_i(100);
        p.store_local(c);
        p.place(&high);
        let keep = p.label();
        p.branch_false(&keep, |p| {
            p.fetch_local(c);
            p.push_f(auto as f32);
            p.binary("ge");
        });
        p.push_i(100);
        p.store_local(c);
        p.place(&keep);
        p.fetch_local(c);
    }

    /// The label of a stat, if the converter has one.
    fn stat_label(stat: Stat) -> Option<u32> {
        use crate::ir::Skill::*;
        Some(match stat {
            Stat::Skill(Speech) => 17,
            Stat::Skill(Barter) => 18,
            Stat::Skill(Doctor) => 19,
            Stat::Skill(Science) => 20,
            Stat::Skill(Traps) => 21,
            Stat::Skill(Lockpick) => 22,
            Stat::Skill(Repair) => 23,
            Stat::Skill(Sneak) => 24,
            Stat::Special(n) if n < 7 => 25 + u32::from(n),
            _ => return None,
        })
    }

    /// One shown option: stores nothing, pushes the gsay arguments and
    /// emits `gsay_option` / `giq_option` (spec 4.3, 4.7).
    fn option(&mut self, p: &mut Proc, plan: &DialoguePlan, t: usize, k: u32, slot: u32) {
        let module = self.ctx.module;
        let end = p.label();
        let procedure = |p: &mut Proc| {
            p.push_p("o_1");
            p.fetch_local(slot);
            p.binary("add");
            p.push_i(1);
            p.binary("sub");
        };
        let members: Vec<usize> = plan.topics[t].picks.iter().flatten().copied().collect();
        for m in members {
            let check = module
                .infos
                .get(&plan.infos[m].id)
                .and_then(|i| i.check.clone());
            let Some(check) = check else {
                continue;
            };
            let mark = p.mark();
            let next = p.label();
            p.branch_false(&next, |p| {
                p.fetch_local(k);
                p.push_i(DialoguePlan::ordinal(m));
                p.binary("eq");
            });
            match check {
                SkillCheck::Percent { difficulty, .. } => {
                    p.push_i(self.list);
                    let open = self.label(6);
                    self.message_str(p, open);
                    self.chance(p, difficulty);
                    p.binary("add");
                    let close = self.label(7);
                    self.message_str(p, close);
                    p.binary("add");
                    self.message_str(p, DialoguePlan::option_id(m));
                    p.binary("add");
                    procedure(p);
                    p.push_i(50);
                    p.host(hw::GSAY_OPTION);
                    self.count_in(Section::Dialogue, Outcome::Compiled, "");
                }
                SkillCheck::Threshold {
                    stat,
                    op,
                    value,
                    shown_when_failed: true,
                    ..
                } => {
                    p.push_i(self.list);
                    match (stat, Self::stat_label(stat)) {
                        (Stat::Skill(crate::ir::Skill::Speech), _) => {
                            let open = self.label(6);
                            self.message_str(p, open);
                        }
                        (_, Some(label)) => {
                            let open = self.label(8);
                            self.message_str(p, open);
                            let name = self.label(label);
                            self.message_str(p, name);
                            p.binary("add");
                            let space = self.msg(" ");
                            self.message_str(p, space);
                            p.binary("add");
                        }
                        (_, None) => {
                            let open = self.label(8);
                            self.message_str(p, open);
                        }
                    }
                    // "[Speech 50] " when met, "[Speech 25/50] " when not.
                    let pass_op = match op {
                        CmpOp::Lt => CmpOp::Ge,
                        CmpOp::Le => CmpOp::Gt,
                        other => other,
                    };
                    let unmet = p.label();
                    let joined = p.label();
                    self.stat_false(p, stat, pass_op, value, &unmet);
                    p.push_i(value);
                    p.binary("add");
                    p.jump(&joined);
                    p.place(&unmet);
                    self.player_stat(p, stat);
                    p.binary("add");
                    let slash = self.label(10);
                    self.message_str(p, slash);
                    p.binary("add");
                    p.push_i(value);
                    p.binary("add");
                    p.place(&joined);
                    let close = self.label(9);
                    self.message_str(p, close);
                    p.binary("add");
                    self.message_str(p, DialoguePlan::option_id(m));
                    p.binary("add");
                    procedure(p);
                    p.push_i(50);
                    p.host(hw::GSAY_OPTION);
                    self.count_in(Section::Dialogue, Outcome::Fallback, "New Vegas failed check shown with a \"25/50\" label and selectable (gap G)");
                }
                SkillCheck::Threshold {
                    stat: Stat::Special(4),
                    op: CmpOp::Ge,
                    value,
                    shown_when_failed: false,
                    ..
                } => {
                    p.push_i(value);
                    p.push_i(self.list);
                    p.push_i(DialoguePlan::option_id(m));
                    procedure(p);
                    p.push_i(50);
                    p.host(hw::GIQ_OPTION);
                    self.count_in(Section::Dialogue, Outcome::Compiled, "");
                }
                SkillCheck::Threshold { .. } => {
                    // A gate inside the pick's condition: a plain option.
                    p.jump(&next);
                }
            }
            p.jump(&end);
            p.place(&next);
            p.release(mark);
        }
        p.push_i(self.list);
        p.fetch_local(k);
        p.push_i(10);
        p.binary("mul");
        p.push_i(10_009);
        p.binary("add");
        procedure(p);
        p.push_i(50);
        p.host(hw::GSAY_OPTION);
        p.place(&end);
    }

    /// An option list: one option per topic whose pick passes.
    fn emit_list(
        &mut self,
        p: &mut Proc,
        plan: &DialoguePlan,
        topics: &[(usize, Option<u32>)],
        k_base: u32,
    ) {
        let slot = p.temp();
        for &(t, given) in topics {
            let mark = p.mark();
            let shown = p.label();
            if let Some(g) = given {
                p.branch_false(&shown, |p| {
                    p.push_i(g as i32);
                    p.host(hw::GLOBAL_VAR);
                });
            }
            let k = p.temp();
            p.call(&format!("p_{}", t + 1));
            p.store_local(k);
            let skip = p.label();
            p.branch_false(&skip, |p| p.fetch_local(k));
            p.fetch_local(slot);
            p.push_i(1);
            p.binary("add");
            p.store_local(slot);
            // d_k<slot> = k
            p.fetch_local(k);
            p.fetch_local(slot);
            p.push_i(k_base as i32 - 1);
            p.binary("add");
            p.core("store_global", 2, 0);
            self.option(p, plan, t, k, slot);
            p.place(&skip);
            p.place(&shown);
            p.release(mark);
        }
    }

    fn info_options(&mut self, p: &mut Proc, plan: &DialoguePlan, n: usize) {
        let flags = self.ctx.module.infos[&plan.infos[n].id].flags;
        if flags.goodbye {
            p.push_i(-2);
            p.push_i(-2);
            p.push_p("x_end");
            p.push_i(50);
            p.host(hw::GSAY_OPTION);
        } else if flags.info_refusal {
            p.call_stmt("relist");
        } else if !plan.infos[n].choices.is_empty() {
            p.call_stmt(&format!("l_{}", n + 1));
        } else {
            p.call_stmt("hub");
        }
    }

    fn end_block(&self, plan: &DialoguePlan, n: usize) -> Option<Vec<crate::ir::Stmt>> {
        self.ctx.module.infos[&plan.infos[n].id]
            .end
            .as_ref()
            .map(|b| b.body.clone())
            .filter(|b| !b.is_empty())
    }

    fn say_info(&mut self, plan: &DialoguePlan, n: usize, pending: u32) {
        let info = self.ctx.module.infos[&plan.infos[n].id].clone();
        self.scope.owner = info.id.form.to_string();
        let mut p = Proc::new(&format!("i_{}", n + 1));
        self.scope.temps.clear();
        if info.flags.say_once
            && let Some(g) = self.ctx.numbers.global(&derived::say_once(&info.id))
        {
            self.set_global(&mut p, g, 1);
        }
        if info.flags.say_once_a_day
            && let Some(g) = self.ctx.numbers.global(&derived::say_day(&info.id))
        {
            self.set_global_with(&mut p, g, |gw, p| gw.game_day(p));
        }
        if self.local_count > locals::TALKED {
            self.set_local(&mut p, locals::TALKED, 1);
        }
        for topic in &info.add_topics {
            if let Some(g) = self.ctx.numbers.global(&derived::topic_given(&topic.form)) {
                self.set_global(&mut p, g, 1);
            }
        }
        if let Some(begin) = info.begin.as_ref().map(|b| b.body.clone()) {
            self.scope.section = Some(Section::Scripts);
            self.block(&mut p, &begin);
            self.scope.section = Some(Section::Dialogue);
        }
        if let Some(end) = self.end_block(plan, n) {
            if info.flags.run_immediately {
                self.scope.section = Some(Section::Scripts);
                self.block(&mut p, &end);
                self.scope.section = Some(Section::Dialogue);
            } else {
                p.push_i(DialoguePlan::ordinal(n));
                p.store_global(pending);
            }
        }
        let parts = plan.infos[n].parts.len();
        p.push_i(self.list);
        p.push_i(DialoguePlan::part_id(n, 0));
        p.host(hw::GSAY_REPLY);
        if parts > 1 {
            let label = self.label(1);
            p.push_i(self.list);
            p.push_i(label);
            p.push_p(&format!("cn_{}_1", n + 1));
            p.push_i(50);
            p.host(hw::GSAY_OPTION);
        } else {
            self.info_options(&mut p, plan, n);
        }
        self.asm.add(p);
        for r in 1..parts {
            let mut p = Proc::new(&format!("cn_{}_{r}", n + 1));
            p.push_i(self.list);
            p.push_i(DialoguePlan::part_id(n, r));
            p.host(hw::GSAY_REPLY);
            if r + 1 < parts {
                let label = self.label(1);
                p.push_i(self.list);
                p.push_i(label);
                p.push_p(&format!("cn_{}_{}", n + 1, r + 1));
                p.push_i(50);
                p.host(hw::GSAY_OPTION);
            } else {
                self.info_options(&mut p, plan, n);
            }
            self.asm.add(p);
        }
    }

    fn challenge(&mut self, plan: &DialoguePlan, n: usize, k_sel: u32) {
        let info = self.ctx.module.infos[&plan.infos[n].id].clone();
        let Some(SkillCheck::Percent {
            difficulty,
            failure,
            remember,
            ..
        }) = info.check.clone()
        else {
            return;
        };
        let mut p = Proc::new(&format!("ch_{}", n + 1));
        self.scope.temps.clear();
        let attempted = self.ctx.numbers.global(&derived::attempted(&info.id));
        let passed = self.ctx.numbers.global(&derived::passed(&info.id));
        // outcome: 1 passed, 0 failed.
        let outcome = p.temp();
        let decide = p.label();
        if let (Some(a), Some(g), Remember::Forever) = (attempted, passed, remember) {
            let roll = p.label();
            p.branch_false(&roll, |p| {
                p.push_i(a as i32);
                p.host(hw::GLOBAL_VAR);
            });
            self.get_global(&mut p, g);
            p.store_local(outcome);
            p.jump(&decide);
            p.place(&roll);
        }
        // One roll (owner decision 1: kept forever).
        p.push_i(1);
        p.push_i(100);
        p.host(hw::RANDOM);
        self.chance(&mut p, difficulty);
        p.binary("le");
        p.store_local(outcome);
        if let Some(a) = attempted {
            self.set_global(&mut p, a, 1);
        }
        if let Some(g) = passed {
            self.set_global_with(&mut p, g, |_, p| p.fetch_local(outcome));
        }
        p.place(&decide);
        let failed = p.label();
        p.branch_false(&failed, |p| p.fetch_local(outcome));
        let xp = self.ctx.settings.speech_xp[usize::from(difficulty.clamp(1, 5) - 1)];
        if xp > 0 {
            p.push_i(xp);
            p.host(hw::GIVE_EXP_POINTS);
        }
        p.call_stmt(&format!("i_{}", n + 1));
        p.ret();
        p.place(&failed);
        let route = match failure {
            FailureRoute::SharedTopic(_) => plan.infos[n].failure.map(Err),
            FailureRoute::NextInfo => {
                let t = plan.infos[n].topic;
                let members: Vec<usize> = plan.topics[t].picks.iter().flatten().copied().collect();
                members
                    .iter()
                    .position(|&m| m == n)
                    .and_then(|i| members.get(i + 1))
                    .copied()
                    .map(Ok)
            }
        };
        match route {
            Some(Err(t)) => {
                let k = p.temp();
                p.call(&format!("p_{}", t + 1));
                p.store_local(k);
                let none = p.label();
                p.branch_false(&none, |p| p.fetch_local(k));
                p.fetch_local(k);
                p.store_global(k_sel);
                p.call_stmt("say");
                p.ret();
                p.place(&none);
                p.call_stmt("relist");
            }
            Some(Ok(next)) => p.call_stmt(&format!("i_{}", next + 1)),
            None => p.call_stmt("relist"),
        }
        self.asm.add(p);
        self.count_in(Section::Dialogue, Outcome::Compiled, "");
    }

    /// Every dialogue procedure of the plan.
    pub fn dialogue_procs(&mut self, plan: &DialoguePlan) {
        self.scope.section = Some(Section::Dialogue);
        self.scope.dialogue = true;
        self.dialogue_messages(plan);
        let pending = self.asm.pvar("d_pending", 0);
        let k_sel = self.asm.pvar("d_k", 0);
        let cur = self.asm.pvar("d_list", 0);
        let k_base = self.asm.pvar("d_k1", 0);
        for s in 2..=plan.slots {
            self.asm.pvar(&format!("d_k{s}"), 0);
        }
        let module = self.ctx.module;
        for t in 0..plan.topics.len() {
            self.pick(plan, t);
        }
        for n in 0..plan.infos.len() {
            self.say_info(plan, n, pending);
            if matches!(
                module.infos[&plan.infos[n].id].check,
                Some(SkillCheck::Percent { .. })
            ) {
                self.challenge(plan, n, k_sel);
            }
        }
        // Lists.
        let mut hub = Proc::new("hub");
        hub.push_i(0);
        hub.store_global(cur);
        let topics = plan.hub.clone();
        self.emit_list(&mut hub, plan, &topics, k_base);
        let goodbye = self.label(2);
        hub.push_i(self.list);
        hub.push_i(goodbye);
        hub.push_p("x_end");
        hub.push_i(50);
        hub.host(hw::GSAY_OPTION);
        self.asm.add(hub);
        for n in 0..plan.infos.len() {
            if plan.infos[n].choices.is_empty() {
                continue;
            }
            let mut p = Proc::new(&format!("l_{}", n + 1));
            p.push_i(DialoguePlan::ordinal(n));
            p.store_global(cur);
            let topics: Vec<(usize, Option<u32>)> =
                plan.infos[n].choices.iter().map(|&t| (t, None)).collect();
            self.emit_list(&mut p, plan, &topics, k_base);
            self.asm.add(p);
        }
        // relist
        let mut p = Proc::new("relist");
        for (n, info) in plan.infos.iter().enumerate() {
            if info.choices.is_empty() {
                continue;
            }
            let next = p.label();
            p.branch_false(&next, |p| {
                p.fetch_global(cur);
                p.push_i(DialoguePlan::ordinal(n));
                p.binary("eq");
            });
            p.call_stmt(&format!("l_{}", n + 1));
            p.ret();
            p.place(&next);
        }
        p.call_stmt("hub");
        self.asm.add(p);
        // Option callbacks.
        for s in 1..=plan.slots {
            let mut p = Proc::new(&format!("o_{s}"));
            p.call_stmt("run_end");
            p.fetch_global(k_base + s as u32 - 1);
            p.store_global(k_sel);
            p.call_stmt("say");
            self.asm.add(p);
        }
        // say
        let mut p = Proc::new("say");
        for n in 0..plan.infos.len() {
            let next = p.label();
            p.branch_false(&next, |p| {
                p.fetch_global(k_sel);
                p.push_i(DialoguePlan::ordinal(n));
                p.binary("eq");
            });
            let challenge = matches!(
                module.infos[&plan.infos[n].id].check,
                Some(SkillCheck::Percent { .. })
            );
            let target = if challenge {
                format!("ch_{}", n + 1)
            } else {
                format!("i_{}", n + 1)
            };
            p.call_stmt(&target);
            p.ret();
            p.place(&next);
        }
        self.asm.add(p);
        // run_end
        let mut p = Proc::new("run_end");
        self.scope.temps.clear();
        let which = p.temp();
        p.fetch_global(pending);
        p.store_local(which);
        p.push_i(0);
        p.store_global(pending);
        self.scope.section = Some(Section::Scripts);
        for n in 0..plan.infos.len() {
            let info = &module.infos[&plan.infos[n].id];
            if info.flags.run_immediately {
                continue;
            }
            let Some(end) = self.end_block(plan, n) else {
                continue;
            };
            self.scope.owner = info.id.form.to_string();
            let next = p.label();
            p.branch_false(&next, |p| {
                p.fetch_local(which);
                p.push_i(DialoguePlan::ordinal(n));
                p.binary("eq");
            });
            self.block(&mut p, &end);
            p.ret();
            p.place(&next);
        }
        self.scope.section = Some(Section::Dialogue);
        self.asm.add(p);
        let mut p = Proc::new("x_end");
        p.call_stmt("run_end");
        self.asm.add(p);
    }

    /// The dialogue part of `talk_p_proc` (spec 4.3).
    pub fn talk(&mut self, p: &mut Proc, plan: &DialoguePlan) {
        let pending = self.asm.pvar("d_pending", 0);
        let k_sel = self.asm.pvar("d_k", 0);
        p.push_i(self.list);
        p.host(hw::SELF_OBJ);
        p.push_i(4);
        p.push_i(-1);
        p.push_i(-1);
        p.host(hw::START_GDIALOG);
        p.host(hw::GSAY_START);
        p.push_i(0);
        p.store_global(pending);
        let done = p.label();
        if let Some(g) = plan.greeting {
            let k = p.temp();
            p.call(&format!("p_{}", g + 1));
            p.store_local(k);
            let none = p.label();
            p.branch_false(&none, |p| p.fetch_local(k));
            p.fetch_local(k);
            p.store_global(k_sel);
            p.call_stmt("say");
            p.jump(&done);
            p.place(&none);
        }
        // No greeting passes (or the speaker has none): the converter's
        // greeting label, then the hub (spec 4.9).
        let label = self.label(crate::texts::GREETING_LABEL);
        p.push_i(self.list);
        p.push_i(label);
        p.host(hw::GSAY_REPLY);
        p.call_stmt("hub");
        p.place(&done);
        p.host(hw::GSAY_END);
        p.call_stmt("run_end");
        p.host(hw::END_DIALOGUE);
    }
}
