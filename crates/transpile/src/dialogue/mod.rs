//! The dialogue compiler (spec 4.1 to 4.10, task T5).
//!
//! [`compile`] turns the module's topics and INFOs into one [`Dialogue`]
//! per speaker base placed in the slice (`Module::dialogues`):
//!
//! - **Static folding** (spec 4.2): each INFO whose speaker set allows the
//!   speaker gets its condition folded with [`cond::fold`] on the
//!   speaker's base record (`GetIsID`, voice type, sex, race, class,
//!   creature, factions unless a script changes them for that base, the
//!   one placed reference); an INFO folded to false is dropped for that
//!   speaker.
//! - **Which topics** are lowered: the `GREETING` topic, the top-level
//!   topics, the topics `AddTopic` (or an INFO's `NAME`) gives, the shared
//!   failure topic and the topics scripts `Say`, then every topic the
//!   passing INFOs of those lead to (`TCLT`, `NAME`, reverse `TCLF`). INFOs
//!   of conversation, combat, detection, miscellaneous, service and radio
//!   topics not so reached are barks and are not lowered (owner decision
//!   14, spec 4.6); topic-type INFOs not reached are counted as unreached.
//! - **Nodes and picks** (spec 4.3, 4.4): nodes in INFO id order give the
//!   ordinals; each topic's picks follow quest priority (higher first),
//!   then file order, with random groups (a run of one quest's random
//!   INFOs up to and including a random-end one).
//! - **Choices** (spec 4.1): `TCLT`, then the reverse `TCLF` links, only
//!   topics the speaker can say, ordered by priority then first
//!   appearance; the hub likewise from the top-level and given topics.
//! - **Checks** (spec 4.7, [`checks`]): a Fallout 3 challenge
//!   (`SkillCheck::Percent`, remembered forever, owner decision 1, the
//!   failure route the shared `SpeechChallengeFailure` topic); a single
//!   player-stat gate becomes `SkillCheck::Threshold`, hidden when failed in
//!   Fallout 3; two INFOs of one topic with opposite gates on the same
//!   value are a New Vegas pair, shown when failed with a "25/50" label and
//!   selectable (owner decision 15). Pairs are found by that shape in either
//!   game. Values compare 1:1 (owner decision 2).
//! - **State** (spec 4.5): one global per say-once INFO, per
//!   say-once-a-day INFO (the day said), per challenge (attempted, passed)
//!   and per given topic ([`once_sub`]).
//!
//! Hello barks are off (owner decision 14); `SimConfig::hello_barks` is
//! recorded, not used.

pub mod checks;

use std::collections::{BTreeMap, BTreeSet};

use crate::build::Pass;
use crate::cond::{self, Facts, SubjectFacts};
use crate::diag::{Category, Diag};
use crate::ir::{
    ActorKind, Cond, Dialogue, DialogueCounts, Expr, FailureRoute, FormKey, Func, Global,
    GlobalPurpose, HubEntry, Info, Node, Pick, RefExpr, ResultOwner, ScriptKind, SimId, SimKind,
    SkillCheck, SpeakerSet, SpeakerTopic, TopicKind, VarTy, global_name, once_sub, visit,
};
use crate::source::FAILURE_TOPIC;

/// The editor id of the greeting topic.
pub const GREETING: &str = "GREETING";

/// Adds the per-speaker dialogues, the checks and the dialogue state
/// globals.
pub fn compile(pass: &mut Pass<'_>) {
    let failure = topic_named(pass, FAILURE_TOPIC);
    let greeting = topic_named(pass, GREETING);
    assign_checks(pass, failure.as_ref());
    let given = given_topics(pass);
    let said = said_topics(pass);
    let dynamic = dynamic_factions(pass);
    let speakers: Vec<FormKey> = pass
        .module
        .actors
        .values()
        .filter(|a| !a.placements.is_empty() && a.kind != ActorKind::Leveled)
        .map(|a| a.base.clone())
        .collect();
    let mut roots: BTreeSet<SimId> = BTreeSet::new();
    roots.extend(greeting.clone());
    roots.extend(failure.clone());
    roots.extend(given.iter().cloned());
    roots.extend(said.iter().cloned());
    roots.extend(
        pass.module
            .topics
            .values()
            .filter(|t| t.top_level && t.kind == TopicKind::Topic)
            .map(|t| t.id.clone()),
    );
    let quest_terms = quest_terms(pass);
    let mut said_by_someone: BTreeSet<SimId> = BTreeSet::new();
    for speaker in speakers {
        let facts = facts(pass, &speaker, &dynamic);
        let Some(dialogue) = speaker_dialogue(
            pass,
            &speaker,
            &facts,
            &quest_terms,
            &roots,
            &given,
            greeting.as_ref(),
            failure.as_ref(),
        ) else {
            continue;
        };
        said_by_someone.extend(dialogue.topics.keys().cloned());
        pass.module.dialogues.insert(speaker, dialogue);
    }
    for topic in &said {
        if !said_by_someone.contains(topic) {
            pass.diag(
                Diag::warning(
                    Category::SayUnresolved,
                    "a script says this topic, and no speaker of the slice can say it",
                )
                .owned_by(topic),
            );
        }
    }
    state_globals(pass, &given);
    missing_topics(pass);
    prune_unsaid(pass);
}

/// A said INFO that links to a topic the MIR does not have: the link is
/// dropped.
fn missing_topics(pass: &mut Pass<'_>) {
    let said: BTreeSet<SimId> = pass
        .module
        .dialogues
        .values()
        .flat_map(|d| d.nodes.iter().map(|n| n.info.clone()))
        .collect();
    let mut found: BTreeSet<(SimId, SimId)> = BTreeSet::new();
    for info in said.iter().filter_map(|i| pass.module.infos.get(i)) {
        for topic in info.choices.iter().chain(&info.add_topics) {
            if !pass.module.topics.contains_key(topic) {
                found.insert((info.id.clone(), topic.clone()));
            }
        }
    }
    for (info, topic) in found {
        pass.diag(
            Diag::warning(
                Category::MissingTopic,
                format!(
                    "links to topic {} that is not in the MIR; dropped",
                    topic.form
                ),
            )
            .owned_by(&info)
            .about(topic.form.as_str()),
        );
    }
}

/// What an INFO's quest adds to its condition: the quest's own conditions
/// and "the quest is running" (an INFO is said only while its quest
/// runs). The running term is decided at build time where the code
/// decides it: a start-enabled quest that no script stops is running; a
/// quest that is not start enabled and that nothing starts (`StartQuest`,
/// `SetStage`, a log entry's next quest) never runs, so its dialogue is
/// folded away.
fn quest_terms(pass: &Pass<'_>) -> BTreeMap<SimId, Cond> {
    let mut started: BTreeSet<FormKey> = BTreeSet::new();
    let mut stopped: BTreeSet<FormKey> = BTreeSet::new();
    for (_, body) in visit::module_bodies(pass.module) {
        for call in visit::calls_in_stmts(body) {
            let Func::Known(id) = &call.func else {
                continue;
            };
            let Some(Expr::Form(quest)) = call.args.first() else {
                continue;
            };
            match id.name() {
                "StartQuest" | "SetStage" => {
                    started.insert(quest.clone());
                }
                "StopQuest" => {
                    stopped.insert(quest.clone());
                }
                _ => {}
            }
        }
    }
    for quest in pass.module.quests.values() {
        for stage in &quest.stages {
            for entry in &stage.entries {
                if let Some(next) = &entry.next_quest {
                    started.insert(next.form.clone());
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    for quest in pass.module.quests.values() {
        let form = &quest.id.form;
        let running = if quest.start_enabled && !stopped.contains(form) {
            Cond::True
        } else if !quest.start_enabled && !started.contains(form) {
            Cond::False
        } else {
            Cond::Cmp {
                lhs: Expr::Call(Box::new(crate::ir::Call {
                    func: Func::Known(
                        crate::ir::FuncId::named("GetQuestRunning")
                            .unwrap_or_else(|| unreachable!("GetQuestRunning is in the table")),
                    ),
                    on: None,
                    args: vec![Expr::Form(form.clone())],
                })),
                op: crate::ir::CmpOp::Eq,
                rhs: Expr::Int(1),
            }
        };
        out.insert(
            quest.id.clone(),
            cond::simplify(&Cond::All(vec![quest.condition.clone(), running])),
        );
    }
    out
}

/// Drops the condition findings of INFOs no speaker says: they were folded
/// away or are barks, and the report counts them as such.
fn prune_unsaid(pass: &mut Pass<'_>) {
    let said: BTreeSet<SimId> = pass
        .module
        .dialogues
        .values()
        .flat_map(|d| d.nodes.iter().map(|n| n.info.clone()))
        .collect();
    pass.module.diagnostics.retain(|d| {
        !(d.category.group() == "condition"
            && d.owner
                .as_ref()
                .is_some_and(|o| o.kind == SimKind::Info && !said.contains(o)))
    });
}

fn topic_named(pass: &Pass<'_>, editor_id: &str) -> Option<SimId> {
    pass.module
        .topics
        .values()
        .find(|t| t.editor_id.eq_ignore_ascii_case(editor_id))
        .map(|t| t.id.clone())
}

/// Fills `Info::check`.
fn assign_checks(pass: &mut Pass<'_>, failure: Option<&SimId>) {
    let first_plugin = pass.source.plugins.first().cloned().unwrap_or_default();
    let mut thresholds: BTreeMap<SimId, checks::StatTerm> = BTreeMap::new();
    let mut found: Vec<(SimId, SkillCheck)> = Vec::new();
    let mut perk_labels: Vec<SimId> = Vec::new();
    for info in pass.module.infos.values() {
        let Some(source) = pass.source.infos.get(&info.id.form) else {
            continue;
        };
        if source.knam_sig.as_deref() == Some("PERK") {
            perk_labels.push(info.id.clone());
        }
        if let Some(check) = checks::percent(source, failure) {
            found.push((info.id.clone(), check));
            continue;
        }
        if let [one] = checks::stat_terms(&source.conditions, &first_plugin).as_slice() {
            thresholds.insert(info.id.clone(), *one);
        }
    }
    // New Vegas pairs: within a topic, in file order, an INFO and the next
    // unpaired one with the opposite comparison of the same value.
    let mut partner: BTreeMap<SimId, SimId> = BTreeMap::new();
    for topic in pass.module.topics.values() {
        let gated: Vec<&SimId> = topic
            .infos
            .iter()
            .filter(|i| thresholds.contains_key(*i))
            .collect();
        for (n, a) in gated.iter().enumerate() {
            if partner.contains_key(*a) {
                continue;
            }
            let ta = thresholds[*a];
            if let Some(b) = gated[n + 1..].iter().find(|b| {
                let tb = thresholds[**b];
                !partner.contains_key(**b)
                    && tb.value_of == ta.value_of
                    && tb.value == ta.value
                    && tb.op == ta.op.negated()
            }) {
                partner.insert((*a).clone(), (*b).clone());
                partner.insert((*b).clone(), (*a).clone());
            }
        }
    }
    for (id, term) in thresholds {
        let label = pass
            .source
            .infos
            .get(&id.form)
            .and_then(|s| s.label)
            .map(crate::ir::ActorValue);
        found.push((
            id.clone(),
            checks::threshold(term, label, partner.get(&id).cloned()),
        ));
    }
    for (id, check) in found {
        if let Some(info) = pass.module.infos.get_mut(&id) {
            info.check = Some(check);
        }
    }
    for id in perk_labels {
        pass.diag(
            Diag::note(
                Category::PerkLabel,
                "the check is labelled by a perk; the perk test is a fallback (HasPerk 0)",
            )
            .owned_by(&id),
        );
    }
}

/// Topics `AddTopic` or an INFO's `NAME` gives.
fn given_topics(pass: &Pass<'_>) -> BTreeSet<SimId> {
    let mut out: BTreeSet<SimId> = BTreeSet::new();
    for info in pass.module.infos.values() {
        out.extend(info.add_topics.iter().cloned());
    }
    for (_, body) in visit::module_bodies(pass.module) {
        for call in visit::calls_in_stmts(body) {
            if matches!(&call.func, Func::Known(id) if id.name() == "AddTopic")
                && let Some(Expr::Form(topic)) = call.args.first()
            {
                out.insert(SimId::of(SimKind::Topic, topic));
            }
        }
    }
    out.retain(|t| pass.module.topics.contains_key(t));
    out
}

/// Topics scripts say with `Say` or `SayTo` (spec 5.9).
fn said_topics(pass: &Pass<'_>) -> BTreeSet<SimId> {
    let mut out = BTreeSet::new();
    for (_, body) in visit::module_bodies(pass.module) {
        for call in visit::calls_in_stmts(body) {
            let Func::Known(id) = &call.func else {
                continue;
            };
            let arg = match id.name() {
                "Say" => call.args.first(),
                "SayTo" => call.args.get(1),
                _ => None,
            };
            if let Some(Expr::Form(topic)) = arg {
                out.insert(SimId::of(SimKind::Topic, topic));
            }
        }
    }
    out
}

/// The factions some script adds, removes or ranks, by actor base (spec
/// 4.2: those terms stay runtime terms).
pub fn dynamic_factions(pass: &Pass<'_>) -> BTreeMap<FormKey, BTreeSet<FormKey>> {
    let mut out: BTreeMap<FormKey, BTreeSet<FormKey>> = BTreeMap::new();
    let all: Vec<FormKey> = pass.module.actors.keys().cloned().collect();
    for script in pass.module.scripts.values() {
        let own: Vec<FormKey> = match &script.kind {
            ScriptKind::Result(ResultOwner::InfoBegin(info) | ResultOwner::InfoEnd(info)) => {
                match pass.module.infos.get(info).map(|i| &i.speakers) {
                    Some(SpeakerSet::Bases(bases)) => bases.clone(),
                    _ => all.clone(),
                }
            }
            _ => pass
                .module
                .actors
                .values()
                .filter(|a| a.script.as_ref() == Some(&script.id))
                .map(|a| a.base.clone())
                .collect(),
        };
        for block in &script.blocks {
            for call in visit::calls_in_stmts(&block.body) {
                let Func::Known(id) = &call.func else {
                    continue;
                };
                if !matches!(
                    id.name(),
                    "AddToFaction" | "RemoveFromFaction" | "SetFactionRank"
                ) {
                    continue;
                }
                let Some(Expr::Form(faction)) = call.args.first() else {
                    continue;
                };
                let bases: Vec<FormKey> = match &call.on {
                    Some(RefExpr::Placed(r)) => {
                        pass.names.ref_base(r).cloned().into_iter().collect()
                    }
                    None | Some(RefExpr::SelfRef) => own.clone(),
                    _ => all.clone(),
                };
                for base in bases {
                    out.entry(base).or_default().insert(faction.clone());
                }
            }
        }
    }
    out
}

/// What is known about a speaker (spec 4.2).
pub fn facts(
    pass: &Pass<'_>,
    base: &FormKey,
    dynamic: &BTreeMap<FormKey, BTreeSet<FormKey>>,
) -> Facts {
    let actor = &pass.module.actors[base];
    Facts {
        subject: Some(SubjectFacts {
            base: base.clone(),
            voice: actor.voice.clone(),
            female: actor.female,
            race: actor.race.clone(),
            class: actor.class.clone(),
            creature: actor.kind == ActorKind::Creature,
            factions: actor.factions.iter().map(|(f, _)| f.clone()).collect(),
            dynamic_factions: dynamic
                .get(base)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default(),
            only_reference: match actor.placements.as_slice() {
                [one] => Some(one.clone()),
                _ => pass.names.unique_ref(base).cloned(),
            },
        }),
        interior: None,
    }
}

fn allows(info: &Info, speaker: &FormKey) -> bool {
    match &info.speakers {
        SpeakerSet::Any => true,
        SpeakerSet::Bases(bases) => bases.contains(speaker),
    }
}

fn is_bark_kind(kind: TopicKind) -> bool {
    !matches!(kind, TopicKind::Topic | TopicKind::Persuasion)
}

#[allow(clippy::too_many_arguments)]
fn speaker_dialogue(
    pass: &mut Pass<'_>,
    speaker: &FormKey,
    facts: &Facts,
    quest_terms: &BTreeMap<SimId, Cond>,
    roots: &BTreeSet<SimId>,
    given: &BTreeSet<SimId>,
    greeting: Option<&SimId>,
    failure: Option<&SimId>,
) -> Option<Dialogue> {
    let module = &*pass.module;
    let mut counts = DialogueCounts::default();
    // Passing INFOs by topic, in the topic's file order, with the folded
    // condition.
    let mut passing: BTreeMap<SimId, Vec<(SimId, Cond)>> = BTreeMap::new();
    for topic in module.topics.values() {
        for info_id in &topic.infos {
            let Some(info) = module.infos.get(info_id) else {
                continue;
            };
            if !allows(info, speaker) {
                continue;
            }
            counts.candidates += 1;
            let whole = match quest_terms.get(&info.quest) {
                Some(Cond::True) | None => info.condition.clone(),
                Some(term) => Cond::All(vec![info.condition.clone(), term.clone()]),
            };
            let folded = cond::fold(&whole, facts);
            if folded == Cond::False {
                counts.folded_away += 1;
                continue;
            }
            passing
                .entry(topic.id.clone())
                .or_default()
                .push((info_id.clone(), folded));
        }
    }
    // The source checks a topic's INFOs quest by quest, higher quest
    // priority first, each quest's in file order (spec 4.1): a stable sort.
    let quest_priority = |info: &SimId| {
        module
            .infos
            .get(info)
            .and_then(|i| module.quests.get(&i.quest))
            .map_or(0, |q| q.priority)
    };
    for infos in passing.values_mut() {
        infos.sort_by_key(|(info, _)| std::cmp::Reverse(quest_priority(info)));
    }
    // Reverse links: topic X leads to the topics whose passing INFOs name
    // X in TCLF.
    let mut leads_to: BTreeMap<SimId, BTreeSet<SimId>> = BTreeMap::new();
    for (topic, infos) in &passing {
        for (info, _) in infos {
            for from in &module.infos[info].link_from {
                leads_to
                    .entry(from.clone())
                    .or_default()
                    .insert(topic.clone());
            }
        }
    }
    // The topics lowered: the closure from the roots.
    let mut lowered: BTreeSet<SimId> = BTreeSet::new();
    let mut pending: Vec<SimId> = roots.iter().rev().cloned().collect();
    while let Some(topic) = pending.pop() {
        if !passing.contains_key(&topic) || !lowered.insert(topic.clone()) {
            continue;
        }
        for (info, _) in &passing[&topic] {
            let info = &module.infos[info];
            pending.extend(info.choices.iter().cloned());
            pending.extend(info.add_topics.iter().cloned());
        }
        if let Some(next) = leads_to.get(&topic) {
            pending.extend(next.iter().cloned());
        }
    }
    for (topic, infos) in &passing {
        if lowered.contains(topic) {
            continue;
        }
        let n = u32::try_from(infos.len()).unwrap_or(u32::MAX);
        match module.topics.get(topic) {
            Some(t) if is_bark_kind(t.kind) => counts.barks += n,
            _ => counts.unreached += n,
        }
    }
    if lowered.is_empty() {
        if counts.barks > 0 {
            pass.diag(barks_diag(pass, speaker, counts.barks));
        }
        return None;
    }
    // Nodes, in INFO id order.
    let mut nodes_by_info: BTreeMap<SimId, Cond> = BTreeMap::new();
    for topic in &lowered {
        for (info, cond) in &passing[topic] {
            nodes_by_info.insert(info.clone(), cond.clone());
        }
    }
    let ordinal: BTreeMap<SimId, u32> = nodes_by_info
        .keys()
        .enumerate()
        .map(|(i, id)| (id.clone(), u32::try_from(i + 1).unwrap_or(u32::MAX)))
        .collect();
    let priority = |t: &SimId| module.topics.get(t).map_or(0.0, |t| t.priority);
    let by_priority = |list: &mut Vec<SimId>| {
        list.sort_by(|a, b| priority(b).total_cmp(&priority(a)));
    };
    // A challenge's shared failure topic links from the challenge's topic
    // (its INFOs' TCLF) so that the failure route can follow it; that link
    // is the route, not a choice for the player (spec 4.7).
    let mut failure_routes: BTreeMap<&SimId, BTreeSet<&SimId>> = BTreeMap::new();
    for info_id in nodes_by_info.keys() {
        let info = &module.infos[info_id];
        if let Some(SkillCheck::Percent {
            failure: FailureRoute::SharedTopic(t),
            ..
        }) = &info.check
        {
            failure_routes.entry(&info.topic).or_default().insert(t);
        }
    }
    let mut nodes = Vec::new();
    let mut late: Vec<Diag> = Vec::new();
    for (info_id, condition) in &nodes_by_info {
        let info = &module.infos[info_id];
        let routes = failure_routes.get(&info.topic);
        let mut choices: Vec<SimId> = Vec::new();
        let linked = leads_to
            .get(&info.topic)
            .into_iter()
            .flatten()
            .filter(|t| !routes.is_some_and(|r| r.contains(t)));
        for topic in info.choices.iter().chain(linked) {
            if lowered.contains(topic) && !choices.contains(topic) {
                choices.push(topic.clone());
            }
        }
        by_priority(&mut choices);
        let failure_topic = match &info.check {
            Some(SkillCheck::Percent {
                failure: FailureRoute::SharedTopic(t),
                ..
            }) => {
                counts.challenges += 1;
                lowered.contains(t).then(|| t.clone())
            }
            Some(SkillCheck::Percent { .. }) => {
                counts.challenges += 1;
                None
            }
            Some(SkillCheck::Threshold { .. }) => {
                counts.thresholds += 1;
                None
            }
            None => None,
        };
        if matches!(info.check, Some(SkillCheck::Percent { .. })) && failure_topic.is_none() {
            let name = module.actors[speaker].editor_id.clone().unwrap_or_default();
            late.push(
                Diag::warning(
                    Category::NoFailureTopic,
                    format!(
                        "{name} has no failure INFO for this challenge; a failure says nothing"
                    ),
                )
                .owned_by(info_id)
                .about(name),
            );
        }
        let prompt = info
            .prompt
            .or_else(|| module.topics.get(&info.topic).and_then(|t| t.name));
        nodes.push(Node {
            ordinal: ordinal[info_id],
            info: info_id.clone(),
            topic: info.topic.clone(),
            condition: condition.clone(),
            prompt,
            choices,
            failure: failure_topic,
        });
    }
    // Picks, in each topic's file order.
    let mut topics = BTreeMap::new();
    for topic in &lowered {
        let mut picks = Vec::new();
        let mut group: Vec<u32> = Vec::new();
        let mut group_quest: Option<&SimId> = None;
        for (info_id, _) in &passing[topic] {
            let info = &module.infos[info_id];
            let n = ordinal[info_id];
            // A random group stays within one quest's INFOs.
            if group_quest != Some(&info.quest) && !group.is_empty() {
                picks.push(close(std::mem::take(&mut group)));
            }
            group_quest = Some(&info.quest);
            if info.flags.random || (info.flags.random_end && !group.is_empty()) {
                group.push(n);
                if info.flags.random_end {
                    picks.push(close(std::mem::take(&mut group)));
                }
            } else {
                if !group.is_empty() {
                    picks.push(close(std::mem::take(&mut group)));
                }
                picks.push(Pick::Info(n));
            }
        }
        if !group.is_empty() {
            picks.push(close(group));
        }
        topics.insert(
            topic.clone(),
            SpeakerTopic {
                topic: topic.clone(),
                picks,
            },
        );
    }
    // The hub.
    let mut hub_topics: Vec<SimId> = lowered
        .iter()
        .filter(|t| Some(*t) != greeting && Some(*t) != failure)
        .filter(|t| {
            module
                .topics
                .get(*t)
                .is_some_and(|topic| topic.top_level && topic.kind == TopicKind::Topic)
                || given.contains(*t)
        })
        .cloned()
        .collect();
    by_priority(&mut hub_topics);
    let hub = hub_topics
        .into_iter()
        .map(|topic| {
            let top = module.topics.get(&topic).is_some_and(|t| t.top_level);
            HubEntry {
                given: (!top).then(|| topic.with_sub(once_sub::GIVEN)),
                topic,
            }
        })
        .collect();
    let greeting_topic = greeting.filter(|g| lowered.contains(*g)).cloned();
    let actor = &module.actors[speaker];
    let editor_id = actor.editor_id.clone().unwrap_or_default();
    pass.module.diagnostics.extend(late);
    if greeting_topic.is_none() {
        pass.diag(
            Diag::warning(
                Category::GreetingLabel,
                "no greeting INFO passes for this speaker: talk opens the hub",
            )
            .owned_by(&SimId::of(SimKind::Actor, speaker))
            .about(editor_id.clone()),
        );
    }
    if counts.barks > 0 {
        pass.diag(barks_diag(pass, speaker, counts.barks));
    }
    Some(Dialogue {
        speaker: speaker.clone(),
        editor_id,
        nodes,
        topics,
        greeting_label: greeting_topic.is_none(),
        greeting: greeting_topic,
        hub,
        counts,
    })
}

fn close(group: Vec<u32>) -> Pick {
    match group.as_slice() {
        [one] => Pick::Info(*one),
        _ => Pick::Random(group),
    }
}

fn barks_diag(pass: &Pass<'_>, speaker: &FormKey, count: u32) -> Diag {
    let name = pass.module.actors[speaker]
        .editor_id
        .clone()
        .unwrap_or_default();
    Diag::note(
        Category::DialogueBark,
        format!("{count} bark INFOs not lowered (owner decision 14)"),
    )
    .owned_by(&SimId::of(SimKind::Actor, speaker))
    .about(name)
}

fn short(form: &FormKey) -> &str {
    form.as_str()
        .rsplit_once(':')
        .map_or(form.as_str(), |(_, id)| id)
}

/// The once-flags of spec 4.5 for every INFO some speaker says, and the
/// given flag of every given topic some speaker says.
fn state_globals(pass: &mut Pass<'_>, given: &BTreeSet<SimId>) {
    let said: BTreeSet<SimId> = pass
        .module
        .dialogues
        .values()
        .flat_map(|d| d.nodes.iter().map(|n| n.info.clone()))
        .collect();
    let lowered_topics: BTreeSet<SimId> = pass
        .module
        .dialogues
        .values()
        .flat_map(|d| d.topics.keys().cloned())
        .collect();
    let add = |pass: &mut Pass<'_>, id: SimId, name: String, initial: i32| {
        pass.module.globals.entry(id.clone()).or_insert(Global {
            id,
            purpose: GlobalPurpose::OnceFlag,
            name: global_name(&name),
            ty: VarTy::Int,
            initial,
        });
    };
    for info_id in said {
        let info = pass.module.infos[&info_id].clone();
        let topic = pass
            .module
            .topics
            .get(&info.topic)
            .map(|t| t.editor_id.clone())
            .unwrap_or_default();
        let tag = format!("{topic}_{}", short(&info_id.form));
        if info.flags.say_once {
            add(
                pass,
                info_id.with_sub(once_sub::SAY_ONCE),
                format!("ONCE_{tag}"),
                0,
            );
        }
        if info.flags.say_once_a_day {
            add(
                pass,
                info_id.with_sub(once_sub::SAY_DAY),
                format!("DAY_{tag}"),
                -1,
            );
        }
        if matches!(info.check, Some(SkillCheck::Percent { .. })) {
            add(
                pass,
                info_id.with_sub(once_sub::ATTEMPTED),
                format!("TRIED_{tag}"),
                0,
            );
            add(
                pass,
                info_id.with_sub(once_sub::PASSED),
                format!("PASSED_{tag}"),
                0,
            );
        }
    }
    for topic in given {
        if !lowered_topics.contains(topic) {
            continue;
        }
        let name = pass
            .module
            .topics
            .get(topic)
            .map(|t| t.editor_id.clone())
            .unwrap_or_default();
        add(
            pass,
            topic.with_sub(once_sub::GIVEN),
            format!("GIVEN_{name}"),
            0,
        );
    }
}
