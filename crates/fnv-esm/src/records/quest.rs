//! Quests (`QUST`): stages with their log entries, and objectives with
//! their targets.

use serde::{Deserialize, Serialize};

use super::{Common, Condition, ScriptBody, typed_record};
use crate::bytes::{Rd, Wr, one_form, one_i16, one_i32, one_u8, push, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A quest's `DATA`: 8 bytes, or 2 in the older form.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestData {
    /// Quest flags, as stored; bit 0 is "start game enabled".
    pub flags: u8,
    /// Priority.
    pub priority: u8,
    /// Script delay in seconds; absent from the 2-byte form.
    pub delay: Option<f32>,
}

/// One log entry of a quest stage: `QSDT`, then its conditions, text,
/// result script and next quest.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestLogEntry {
    /// Entry flags (`QSDT`), as stored; bit 0 is "complete quest", bit 1
    /// "fail quest". Every entry of a valid record starts with a `QSDT`;
    /// `None` marks an entry the decoder had to open without one.
    pub flags: Option<u8>,
    /// Conditions for this entry (`CTDA`).
    pub conditions: Vec<Condition>,
    /// The log text (`CNAM`).
    pub text: Option<String>,
    /// The result script; empty when the entry has none.
    pub script: ScriptBody,
    /// The quest started next (`NAM0`).
    pub next_quest: Option<FormId>,
}

impl QuestLogEntry {
    /// An entry with log text and no conditions.
    pub fn new(text: &str) -> Self {
        Self {
            flags: Some(0),
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }
}

/// A quest stage (`INDX`) and its log entries.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestStage {
    /// The stage index.
    pub index: i16,
    /// Log entries, in file order.
    pub entries: Vec<QuestLogEntry>,
}

/// A target of a quest objective (`QSTA`) and its conditions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestTarget {
    /// The target reference.
    pub target: FormId,
    /// Flags, as stored; bit 0 is "compass marker ignores locks".
    pub flags: u8,
    /// Conditions for this target (`CTDA`).
    pub conditions: Vec<Condition>,
}

/// A quest objective (`QOBJ`), its text and its targets.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuestObjective {
    /// The objective index.
    pub index: i32,
    /// The objective text (`NNAM`).
    pub description: Option<String>,
    /// Targets, in file order.
    pub targets: Vec<QuestTarget>,
}

/// A quest (`QUST`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Quest {
    /// The common fields. `script` is the quest script.
    pub common: Common,
    /// The icon path (`ICON`).
    pub icon: Option<String>,
    /// Flags, priority and delay (`DATA`).
    pub data: Option<QuestData>,
    /// The quest's own conditions: every `CTDA` before the first stage.
    pub conditions: Vec<Condition>,
    /// Stages, in file order.
    pub stages: Vec<QuestStage>,
    /// Objectives, in file order.
    pub objectives: Vec<QuestObjective>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

/// Which part of the record the decoder is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Header,
    Stage,
    Objective,
}

impl Quest {
    /// A quest with an editor id and a display name.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, name: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name),
            data: Some(QuestData {
                flags: 0,
                priority: 0,
                delay: Some(0.0),
            }),
            ..Self::default()
        }
    }

    /// The log entry a stage subrecord belongs to: the last one of the last
    /// stage, opened if the stage has none yet.
    fn entry(&mut self) -> Option<&mut QuestLogEntry> {
        let stage = self.stages.last_mut()?;
        if stage.entries.is_empty() {
            stage.entries.push(QuestLogEntry::default());
        }
        stage.entries.last_mut()
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        let mut section = Section::Header;
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match (&s.sig.0, section) {
                (b"INDX", _) => {
                    let opened = push(
                        &mut self.stages,
                        one_i16(d).map(|index| QuestStage {
                            index,
                            entries: Vec::new(),
                        }),
                    );
                    if opened {
                        section = Section::Stage;
                    }
                    opened
                }
                (b"QOBJ", _) => {
                    let opened = push(
                        &mut self.objectives,
                        one_i32(d).map(|index| QuestObjective {
                            index,
                            ..QuestObjective::default()
                        }),
                    );
                    if opened {
                        section = Section::Objective;
                    }
                    opened
                }
                (b"ICON", Section::Header) => set(&mut self.icon, zstring(d)),
                (b"DATA", Section::Header) => set(&mut self.data, parse_data(d)),
                (b"CTDA", Section::Header) => push(&mut self.conditions, Condition::parse(d)),
                (b"QSDT", Section::Stage) => match (self.stages.last_mut(), one_u8(d)) {
                    (Some(stage), Some(flags)) => {
                        stage.entries.push(QuestLogEntry {
                            flags: Some(flags),
                            ..QuestLogEntry::default()
                        });
                        true
                    }
                    _ => false,
                },
                (b"CTDA", Section::Stage) => match Condition::parse(d) {
                    Some(condition) => match self.entry() {
                        Some(entry) => {
                            entry.conditions.push(condition);
                            true
                        }
                        None => false,
                    },
                    None => false,
                },
                (b"CNAM", Section::Stage) => match self.entry() {
                    Some(entry) => set(&mut entry.text, zstring(d)),
                    None => false,
                },
                (b"NAM0", Section::Stage) => match one_form(d) {
                    Some(form) => match self.entry() {
                        Some(entry) => set(&mut entry.next_quest, Some(form)),
                        None => false,
                    },
                    None => false,
                },
                (
                    b"SCHR" | b"SCDA" | b"SCTX" | b"SLSD" | b"SCVR" | b"SCRO" | b"SCRV",
                    Section::Stage,
                ) => match self.entry() {
                    Some(entry) => entry.script.take(s),
                    None => false,
                },
                (b"NNAM", Section::Objective) => match self.objectives.last_mut() {
                    Some(objective) => set(&mut objective.description, zstring(d)),
                    None => false,
                },
                (b"QSTA", Section::Objective) => match self.objectives.last_mut() {
                    Some(objective) => push(&mut objective.targets, parse_qsta(d)),
                    None => false,
                },
                (b"CTDA", Section::Objective) => {
                    let target = self
                        .objectives
                        .last_mut()
                        .and_then(|objective| objective.targets.last_mut());
                    match target {
                        Some(target) => push(&mut target.conditions, Condition::parse(d)),
                        None => false,
                    }
                }
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.icon);
        out.opt(
            b"DATA",
            self.data.map(|d| {
                let w = Wr::default().u8(d.flags).u8(d.priority);
                match d.delay {
                    Some(delay) => w.zeros(2).f32(delay),
                    None => w,
                }
                .done()
            }),
        );
        for condition in &self.conditions {
            out.raw(b"CTDA", condition.bytes());
        }
        for stage in &self.stages {
            out.raw(b"INDX", stage.index.to_le_bytes().to_vec());
            for entry in &stage.entries {
                out.u8(b"QSDT", entry.flags);
                for condition in &entry.conditions {
                    out.raw(b"CTDA", condition.bytes());
                }
                out.zstr(b"CNAM", &entry.text);
                entry.script.emit(out);
                out.form(b"NAM0", entry.next_quest);
            }
        }
        for objective in &self.objectives {
            out.i32(b"QOBJ", Some(objective.index));
            out.zstr(b"NNAM", &objective.description);
            for target in &objective.targets {
                out.raw(
                    b"QSTA",
                    Wr::default()
                        .form(target.target)
                        .u8(target.flags)
                        .zeros(3)
                        .done(),
                );
                for condition in &target.conditions {
                    out.raw(b"CTDA", condition.bytes());
                }
            }
        }
    }
}

typed_record!(Quest, QUST);

fn parse_data(data: &[u8]) -> Option<QuestData> {
    if !matches!(data.len(), 2 | 8) {
        return None;
    }
    let mut rd = Rd::new(data);
    let flags = rd.u8()?;
    let priority = rd.u8()?;
    let delay = rd.skip(2).and_then(|()| rd.f32());
    Some(QuestData {
        flags,
        priority,
        delay,
    })
}

fn parse_qsta(data: &[u8]) -> Option<QuestTarget> {
    let mut rd = Rd::exact(data, 8)?;
    Some(QuestTarget {
        target: rd.form()?,
        flags: rd.u8()?,
        conditions: Vec::new(),
    })
}
