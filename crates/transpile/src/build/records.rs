//! Source records to IR records: a field-for-field translation, with
//! texts interned, conditions lowered and embedded scripts compiled. The
//! meaning (speaker folding, checks, quest state, runners) is the later
//! modules'.

use std::collections::BTreeSet;

use super::Builder;
use crate::cond::{self, CondUses};
use crate::diag::{Category, Diag};
use crate::ir::{
    Cond, FormKey, Info, InfoFlags, Location, LogEntry, MenuItem, Note, NoteBody, Objective,
    Package, PackageEvent, PackageKind, Quest, RefUses, Response, ResultOwner, Schedule,
    ScriptKind, SimId, SimKind, SpeakerSet, Stage, Target, Terminal, TextRole, Topic, TopicKind,
    script_sub,
};
use crate::source::{CondValue, SourceCondition};

/// `INFO` flag bits (first byte, then second byte), as the editor names
/// them. Bits 0 to 2 are confirmed by the decoder; the rest follow the
/// public record documentation and spec 1.3 (task T1 adds named
/// accessors).
mod info_flag {
    pub const GOODBYE: u16 = 0x0001;
    pub const RANDOM: u16 = 0x0002;
    pub const SAY_ONCE: u16 = 0x0004;
    pub const RUN_IMMEDIATELY: u16 = 0x0008;
    pub const INFO_REFUSAL: u16 = 0x0010;
    pub const RANDOM_END: u16 = 0x0020;
    pub const SAY_ONCE_A_DAY: u16 = 0x0100;
}

/// `DIAL` flag `0x02`: top level (spec 4.1).
const TOP_LEVEL: u8 = 0x02;

/// The bases a condition list names as the subject with `GetIsID == 1`
/// (or `!= 0`).
fn speaker_terms(conditions: &[SourceCondition]) -> Vec<FormKey> {
    conditions
        .iter()
        .filter(|c| c.function == 72 && c.run_on == 0)
        .filter(|c| match (&c.value, c.comparison.as_str()) {
            (CondValue::Literal(v), "==") => *v == 1.0,
            (CondValue::Literal(v), "!=") => *v == 0.0,
            (CondValue::Literal(v), ">=") => *v == 1.0,
            _ => false,
        })
        .filter_map(|c| c.param1_form.clone())
        .collect()
}

impl Builder<'_> {
    pub(super) fn lower_cond(&mut self, conditions: &[SourceCondition], owner: &SimId) -> Cond {
        let mut uses = CondUses::default();
        let cond = cond::lower(conditions, owner, &self.names, &mut uses);
        self.uses.globals.extend(uses.globals);
        self.uses.quests.extend(uses.quests);
        for r in uses.refs {
            self.uses.mark(&r, RefUses::CONDITION);
        }
        self.module.diagnostics.extend(uses.diags);
        cond
    }

    pub(super) fn object_scripts(&mut self) {
        let source = self.source;
        let scripts: BTreeSet<FormKey> = source
            .bases
            .values()
            .filter_map(|b| b.script.clone())
            .collect();
        for script in scripts {
            self.compile_record(&script, None);
        }
    }

    pub(super) fn packages(&mut self) {
        let source = self.source;
        for (form, package) in &source.packages {
            let id = SimId::of(SimKind::Package, form);
            let editor_id = package.editor_id.clone().unwrap_or_default();
            let condition = self.lower_cond(&package.conditions, &id);
            let mut events = Vec::new();
            for (event, body) in &package.events {
                let sub = match event {
                    PackageEvent::Begin => script_sub::PACKAGE_BEGIN,
                    PackageEvent::End => script_sub::PACKAGE_END,
                    PackageEvent::Change => script_sub::PACKAGE_CHANGE,
                };
                let script = body.as_ref().map(|body| {
                    self.compile_body(
                        SimId::result_script(form, sub),
                        &editor_id,
                        ScriptKind::Result(ResultOwner::PackageEvent(id.clone(), *event)),
                        body,
                        None,
                        None,
                    )
                });
                events.push((*event, script));
            }
            let location = package.location.as_ref().and_then(|l| {
                let form = l.form.clone();
                if let Some(f) = &form {
                    self.uses.mark(f, RefUses::PACKAGE);
                }
                Some(match l.location_type {
                    0 => Location::NearRef {
                        r: form?,
                        radius: l.radius,
                    },
                    1 => Location::InCell {
                        cell: form?,
                        radius: l.radius,
                    },
                    2 => Location::NearCurrent { radius: l.radius },
                    3 => Location::NearEditor { radius: l.radius },
                    4 => Location::ObjectId(form?),
                    5 => Location::ObjectType(l.value),
                    6 => Location::NearLinkedRef { radius: l.radius },
                    7 => Location::AtPackageLocation,
                    _ => return None,
                })
            });
            let target = package.target.as_ref().and_then(|t| {
                let form = t.form.clone();
                if let Some(f) = &form {
                    self.uses.mark(f, RefUses::PACKAGE);
                }
                Some(match t.target_type {
                    0 => Target::Ref(form?),
                    1 => Target::Object(form?),
                    2 => Target::ObjectType(t.value),
                    3 => Target::LinkedRef,
                    _ => return None,
                })
            });
            let schedule = package
                .schedule
                .map_or(Schedule::ANY, |(m, d, date, h, dur)| Schedule {
                    month: m,
                    day_of_week: d,
                    date,
                    hour: h,
                    duration_hours: dur,
                });
            let kind = PackageKind::from_code(package.package_type);
            self.module.packages.insert(
                id.clone(),
                Package {
                    id,
                    editor_id,
                    kind,
                    flags: package.flags,
                    behaviour_flags: package.behaviour_flags,
                    type_flags: package.type_flags,
                    location,
                    target,
                    schedule,
                    condition,
                    events,
                    runner: kind.runner(),
                },
            );
        }
    }

    pub(super) fn infos(&mut self) {
        let source = self.source;
        for (form, info) in &source.infos {
            let id = SimId::of(SimKind::Info, form);
            let mut bases: Vec<FormKey> = speaker_terms(&info.conditions);
            bases.extend(info.speaker.clone());
            bases.sort();
            bases.dedup();
            let speakers = if bases.is_empty() {
                SpeakerSet::Any
            } else {
                SpeakerSet::Bases(bases.clone())
            };
            let implicit = match bases.as_slice() {
                [one] => self.speaker_vars(one),
                _ => None,
            };
            let condition = self.lower_cond(&info.conditions, &id);
            let quest = match &info.quest {
                Some(q) => {
                    self.uses.quests.insert(q.clone());
                    SimId::of(SimKind::Quest, q)
                }
                None => {
                    self.diag(
                        Diag::warning(Category::MissingRecord, "INFO without a quest")
                            .owned_by(&id),
                    );
                    SimId::of(SimKind::Quest, &FormKey::converter())
                }
            };
            let flags = InfoFlags {
                goodbye: info.flags & info_flag::GOODBYE != 0,
                random: info.flags & info_flag::RANDOM != 0,
                random_end: info.flags & info_flag::RANDOM_END != 0,
                say_once: info.flags & info_flag::SAY_ONCE != 0,
                say_once_a_day: info.flags & info_flag::SAY_ONCE_A_DAY != 0,
                run_immediately: info.flags & info_flag::RUN_IMMEDIATELY != 0,
                info_refusal: info.flags & info_flag::INFO_REFUSAL != 0,
            };
            let responses = info
                .responses
                .iter()
                .map(|r| Response {
                    number: r.number,
                    text: self.texts.intern(&id, TextRole::Response, &r.text),
                    emotion: (r.emotion_type, r.emotion_value),
                })
                .collect();
            let prompt = self
                .texts
                .intern_opt(&id, TextRole::Prompt, info.prompt.as_deref());
            let editor_id = form.as_str().to_owned();
            let begin = info.begin.as_ref().map(|body| {
                self.compile_body(
                    SimId::result_script(form, script_sub::INFO_BEGIN),
                    &editor_id,
                    ScriptKind::Result(ResultOwner::InfoBegin(id.clone())),
                    body,
                    None,
                    implicit.clone(),
                )
            });
            let end = info.end.as_ref().map(|body| {
                self.compile_body(
                    SimId::result_script(form, script_sub::INFO_END),
                    &editor_id,
                    ScriptKind::Result(ResultOwner::InfoEnd(id.clone())),
                    body,
                    None,
                    implicit.clone(),
                )
            });
            let block = |s: Option<SimId>, module: &crate::ir::Module| {
                s.and_then(|s| module.scripts.get(&s))
                    .and_then(|script| script.blocks.first().cloned())
            };
            let topics = |list: &[FormKey]| -> Vec<SimId> {
                list.iter().map(|t| SimId::of(SimKind::Topic, t)).collect()
            };
            let info_ir = Info {
                id: id.clone(),
                topic: SimId::of(SimKind::Topic, &info.topic),
                quest,
                speakers,
                condition,
                flags,
                responses,
                prompt,
                choices: topics(&info.choices),
                link_from: topics(&info.link_from),
                add_topics: topics(&info.add_topics),
                begin: block(begin, &self.module),
                end: block(end, &self.module),
                check: None,
            };
            self.module.infos.insert(id, info_ir);
        }
    }

    pub(super) fn terminals(&mut self) {
        let source = self.source;
        for (form, terminal) in &source.terminals {
            let id = SimId::of(SimKind::Terminal, form);
            let editor_id = terminal.editor_id.clone().unwrap_or_default();
            let header =
                self.texts
                    .intern_opt(&id, TextRole::TerminalHeader, terminal.header.as_deref());
            let password = terminal.password_note.as_ref().map(|n| {
                self.uses.notes.insert(n.clone());
                SimId::of(SimKind::Note, n)
            });
            let mut items = Vec::new();
            for (seq, item) in terminal.items.iter().enumerate() {
                let seq = u32::try_from(seq).unwrap_or(u32::MAX);
                let item_id = SimId::new(SimKind::MenuItem, form.clone(), seq);
                let text = self.texts.intern(
                    &item_id,
                    TextRole::MenuItem,
                    item.text.as_deref().unwrap_or_default(),
                );
                let result =
                    self.texts
                        .intern_opt(&item_id, TextRole::MenuResult, item.result.as_deref());
                let condition = self.lower_cond(&item.conditions, &item_id);
                let script = item.script.as_ref().map(|body| {
                    self.compile_body(
                        SimId::result_script(form, script_sub::MENU_ITEM + seq),
                        &editor_id,
                        ScriptKind::Result(ResultOwner::MenuItem(item_id.clone())),
                        body,
                        None,
                        None,
                    )
                });
                let show_note = item.display_note.as_ref().map(|n| {
                    self.uses.notes.insert(n.clone());
                    SimId::of(SimKind::Note, n)
                });
                items.push(MenuItem {
                    id: item_id,
                    text,
                    result,
                    add_note: item.flags & 0x01 != 0,
                    show_note,
                    submenu: item
                        .sub_terminal
                        .as_ref()
                        .map(|t| SimId::of(SimKind::Terminal, t)),
                    script,
                    condition,
                });
            }
            self.module.terminals.insert(
                id.clone(),
                Terminal {
                    id,
                    header,
                    difficulty: terminal.difficulty,
                    flags: terminal.flags,
                    server: terminal.server,
                    password,
                    items,
                },
            );
        }
    }

    /// The quests the slice names, then `quest_depth` more levels.
    pub(super) fn quests(&mut self) {
        let mut done: BTreeSet<FormKey> = BTreeSet::new();
        let mut level = 0;
        loop {
            let pending: Vec<FormKey> = self
                .uses
                .quests
                .iter()
                .filter(|q| !done.contains(*q))
                .cloned()
                .collect();
            if pending.is_empty() {
                break;
            }
            if level > self.config.quest_depth {
                for quest in pending {
                    let diag = Diag::note(
                        Category::QuestDepth,
                        format!("quest {quest} is named beyond the quest depth; not built"),
                    )
                    .about(quest.as_str());
                    self.diag(diag);
                }
                break;
            }
            for quest in pending {
                done.insert(quest.clone());
                self.quest(&quest);
            }
            level += 1;
        }
    }

    fn quest(&mut self, form: &FormKey) {
        let source = self.source;
        let id = SimId::of(SimKind::Quest, form);
        let Some(quest) = source.quests.get(form) else {
            let diag = Diag::warning(
                Category::MissingRecord,
                format!("quest {form} is not in the MIR"),
            )
            .about(form.as_str());
            self.diag(diag);
            return;
        };
        let editor_id = quest.editor_id.clone().unwrap_or_default();
        let name = self
            .texts
            .intern_opt(&id, TextRole::QuestName, quest.name.as_deref());
        let script = quest
            .script
            .clone()
            .and_then(|s| self.compile_record(&s, Some(form)));
        let vars = self
            .names
            .quest_vars(form)
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        let mut stages = Vec::new();
        for stage in &quest.stages {
            let stage_id = SimId::stage(form, stage.index);
            let mut entries = Vec::new();
            for (number, entry) in stage.entries.iter().enumerate() {
                let number = u32::try_from(number).unwrap_or(u32::MAX);
                let condition = self.lower_cond(&entry.conditions, &stage_id);
                let text =
                    self.texts
                        .intern_opt(&stage_id, TextRole::LogEntry, entry.text.as_deref());
                let script = entry.script.as_ref().map(|body| {
                    self.compile_body(
                        SimId::stage_script(form, stage.index, number),
                        &editor_id,
                        ScriptKind::Result(ResultOwner::Stage(stage_id.clone())),
                        body,
                        Some(form),
                        None,
                    )
                });
                let next_quest = entry.next_quest.as_ref().map(|q| {
                    self.uses.quests.insert(q.clone());
                    SimId::of(SimKind::Quest, q)
                });
                entries.push(LogEntry {
                    complete: entry.flags & 0x01 != 0,
                    fail: entry.flags & 0x02 != 0,
                    condition,
                    text,
                    script,
                    next_quest,
                });
            }
            stages.push(Stage {
                id: stage_id,
                index: stage.index,
                entries,
            });
        }
        stages.sort_by_key(|s| s.index);
        let mut objectives = Vec::new();
        for objective in &quest.objectives {
            let objective_id = SimId::objective(form, objective.index);
            let text = self.texts.intern_opt(
                &objective_id,
                TextRole::Objective,
                objective.text.as_deref(),
            );
            let mut targets = Vec::new();
            for (target, conditions) in &objective.targets {
                self.uses.mark(target, RefUses::QUEST_TARGET);
                targets.push((target.clone(), self.lower_cond(conditions, &objective_id)));
            }
            objectives.push(Objective {
                id: objective_id,
                index: objective.index,
                text,
                targets,
            });
        }
        objectives.sort_by_key(|o| o.index);
        self.module.quests.insert(
            id.clone(),
            Quest {
                id,
                editor_id,
                name,
                start_enabled: quest.flags & 0x01 != 0,
                priority: quest.priority,
                delay_seconds: quest.delay,
                script,
                vars,
                stages,
                objectives,
            },
        );
    }

    pub(super) fn topics(&mut self) {
        let source = self.source;
        for (form, topic) in &source.topics {
            let id = SimId::of(SimKind::Topic, form);
            let name = self
                .texts
                .intern_opt(&id, TextRole::TopicName, topic.name.as_deref());
            let kind = TopicKind::from_code(topic.topic_type).unwrap_or(TopicKind::Misc);
            let mut infos: Vec<(&(u32, u64), SimId)> = self
                .source
                .infos
                .iter()
                .filter(|(_, info)| &info.topic == form)
                .map(|(info_form, info)| (&info.order, SimId::of(SimKind::Info, info_form)))
                .collect();
            infos.sort();
            self.module.topics.insert(
                id.clone(),
                Topic {
                    id,
                    editor_id: topic.editor_id.clone().unwrap_or_default(),
                    kind,
                    name,
                    priority: topic.priority,
                    top_level: topic.flags & TOP_LEVEL != 0,
                    infos: infos.into_iter().map(|(_, id)| id).collect(),
                },
            );
        }
    }

    pub(super) fn notes(&mut self) {
        let source = self.source;
        let placed_notes = self
            .source
            .bases
            .iter()
            .filter(|(_, b)| b.sig == "NOTE")
            .map(|(k, _)| k.clone());
        let wanted: BTreeSet<FormKey> = self
            .uses
            .notes
            .iter()
            .cloned()
            .chain(placed_notes)
            .collect();
        for form in wanted {
            let id = SimId::of(SimKind::Note, &form);
            let Some(note) = source.notes.get(&form) else {
                let diag = Diag::warning(
                    Category::MissingRecord,
                    format!("note {form} is not in the MIR"),
                )
                .about(form.as_str());
                self.diag(diag);
                continue;
            };
            let title = self
                .texts
                .intern_opt(&id, TextRole::NoteTitle, note.title.as_deref());
            let body = match note.note_type {
                3 => NoteBody::Voice {
                    topic: note.topic.as_ref().map(|t| SimId::of(SimKind::Topic, t)),
                },
                2 => NoteBody::Image(note.texture.clone().unwrap_or_default()),
                0 => NoteBody::Sound(
                    note.sound
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                ),
                _ => NoteBody::Text(self.texts.intern(
                    &id,
                    TextRole::NoteBody,
                    note.text.as_deref().unwrap_or_default(),
                )),
            };
            self.module
                .notes
                .insert(id.clone(), Note { id, title, body });
        }
    }
}
