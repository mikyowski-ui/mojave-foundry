//! Typed queries over the logic tables of `records.sqlite`: terminals,
//! notes, AI packages, the packages actors use, conditions, and the set of
//! placed references that something points at.
//!
//! These tables arrived with [`crate::RECORDS_REVISION`] 1; the globals,
//! game settings, messages, factions, actor AI data and factions, info
//! links, package events, script contexts and decode skips with revision 2.
//! On a file written before a table every method that reads it returns
//! [`MirError::MissingTable`]; extracting again rebuilds the file. The
//! columns revision 2 added to older tables (`conditions`, `infos`,
//! `packages`) read as absent on an older file.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::error::MirError;
use crate::paths::FormKey;
use crate::reader::Records;

use fnv_esm::PackageData;

pub(crate) fn form(text: Option<String>) -> Option<FormKey> {
    text.and_then(|t| t.parse().ok())
}

/// One condition, as `conditions` stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionRow {
    /// Where in the owner it applies: `"quest"`, `"info"`, `"item 2"`,
    /// `"package"`, `"stage 10 entry 0"`, `"objective 5 target 0"`, ...
    pub context: String,
    /// The function index.
    pub function: u32,
    /// `"=="`, `"!="`, `">"`, `">="`, `"<"`, `"<="`.
    pub comparison: String,
    /// The literal compared against, when it is not a global.
    pub value: Option<f64>,
    /// The global compared against.
    pub global: Option<FormKey>,
    /// The first parameter, as stored.
    pub param1: u32,
    /// The second parameter, as stored.
    pub param2: u32,
    /// What the function runs on, as stored (2: [`Self::reference`]).
    pub run_on: u32,
    /// The reference the function runs on.
    pub reference: Option<FormKey>,
    /// OR-ed with the next condition.
    pub is_or: bool,
    /// The placed reference the first parameter names, for the functions
    /// that take one (`fnv_esm::REFERENCE_PARAMETER_FUNCTIONS`) when it
    /// resolves to one; `None` otherwise. Revision 2.
    pub param1_ref: Option<FormKey>,
    /// The placed reference the second parameter names, as
    /// [`Self::param1_ref`]. Revision 2.
    pub param2_ref: Option<FormKey>,
}

/// One menu item of a terminal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalItemRow {
    /// Position in the menu, from 0.
    pub seq: u32,
    /// The menu text.
    pub text: Option<String>,
    /// The text shown when the item is chosen.
    pub result_text: Option<String>,
    /// Item flags, as stored.
    pub flags: Option<u8>,
    /// The note the item shows.
    pub display_note: Option<FormKey>,
    /// The terminal the item opens.
    pub sub_terminal: Option<FormKey>,
    /// Source text of the item's script.
    pub script_source: Option<String>,
    /// Conditions for showing the item.
    pub conditions: Vec<ConditionRow>,
}

/// A terminal and its menu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalRow {
    /// The terminal record.
    pub form: FormKey,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Model path, normalised.
    pub model: Option<String>,
    /// The terminal's own script.
    pub script: Option<FormKey>,
    /// The header text above the menu.
    pub header_text: Option<String>,
    /// Hacking difficulty, as stored.
    pub difficulty: Option<u8>,
    /// Terminal flags, as stored.
    pub flags: Option<u8>,
    /// Server type, as stored.
    pub server_type: Option<u8>,
    /// The note holding the password.
    pub password_note: Option<FormKey>,
    /// The looping sound.
    pub sound: Option<FormKey>,
    /// The menu items, in order.
    pub items: Vec<TerminalItemRow>,
}

/// A note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRow {
    /// The note record.
    pub form: FormKey,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Title.
    pub name: Option<String>,
    /// Model path, normalised.
    pub model: Option<String>,
    /// Note type: 0 sound, 1 text, 2 image, 3 voice.
    pub note_type: Option<u8>,
    /// The text, for any but a voice note.
    pub text: Option<String>,
    /// A voice note's topic.
    pub topic: Option<FormKey>,
    /// The sound; for a voice note, the speaker.
    pub sound: Option<FormKey>,
    /// An image note's texture, normalised.
    pub texture: Option<String>,
    /// Quests the note belongs to.
    pub quests: Vec<FormKey>,
}

/// A package location (`PLDT` or `PLD2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLocationRow {
    /// Location type, as stored (0 near a reference, 1 in a cell, 4 an
    /// object id, ...).
    pub location_type: i32,
    /// The form, for the types whose value is one.
    pub form: Option<FormKey>,
    /// The raw value.
    pub value: u32,
    /// Radius, game units.
    pub radius: i32,
}

/// A package target (`PTDT` or `PTD2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTargetRow {
    /// Target type, as stored (0 a specific reference, 1 an object id, ...).
    pub target_type: i32,
    /// The form, for the types whose value is one.
    pub form: Option<FormKey>,
    /// The raw value.
    pub value: u32,
    /// Count or distance.
    pub count: i32,
}

/// An AI package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageRow {
    /// The package record.
    pub form: FormKey,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Package type, as stored.
    pub package_type: Option<u8>,
    /// The type's name (`fnv_esm::PackageData::type_name`).
    pub type_name: Option<String>,
    /// General flags, as stored.
    pub flags: Option<u32>,
    /// The schedule.
    pub schedule: Option<fnv_esm::PackageSchedule>,
    /// The location.
    pub location: Option<PackageLocationRow>,
    /// The second location.
    pub location2: Option<PackageLocationRow>,
    /// The target.
    pub target: Option<PackageTargetRow>,
    /// The second target.
    pub target2: Option<PackageTargetRow>,
    /// The dialogue topic of a dialogue package.
    pub dialogue_topic: Option<FormKey>,
    /// The combat style.
    pub combat_style: Option<FormKey>,
    /// Conditions.
    pub conditions: Vec<ConditionRow>,
    /// Event scripts with source text: `("on begin" | "on end" |
    /// "on change", source)`.
    pub scripts: Vec<(String, String)>,
    /// The size of the stored `PKDT`: 8 or 12. Revision 2.
    pub pkdt_size: Option<u8>,
    /// The behaviour flags, `None` when `PKDT` has none or stores the
    /// uninitialised fill (`fnv_esm::PackageData::behaviour_flags`).
    /// Revision 2.
    pub behaviour_flags: Option<u16>,
    /// The behaviour word as stored, fill included. Revision 2.
    pub behaviour_flags_raw: Option<u16>,
    /// The type-specific flags; `None` in the 8-byte form. Revision 2.
    pub type_flags: Option<u16>,
    /// The three event blocks that are present, in order, with their idle
    /// and topic and script source. Revision 2.
    pub events: Vec<PackageEventRow>,
}

impl PackageRow {
    /// The `PKDT` as `fnv_esm` reads it, for its named flag accessors
    /// (`must_reach_location`, `always_run`, `sandbox_flag_names`, ...).
    /// `None` when the package has none.
    pub fn data(&self) -> Option<PackageData> {
        let (flags, package_type) = (self.flags?, self.package_type?);
        let behaviour = self.behaviour_flags_raw.unwrap_or(0);
        Some(match self.type_flags {
            Some(type_flags) => PackageData::new(package_type, flags, behaviour, type_flags),
            None => PackageData::short(package_type, flags, behaviour),
        })
    }
}

/// One event block of a package (`POBA`, `POEA`, `POCA`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEventRow {
    /// `"on begin"`, `"on end"` or `"on change"`.
    pub event: String,
    /// The idle played (`INAM`).
    pub idle: Option<FormKey>,
    /// The topic said (`TNAM`).
    pub topic: Option<FormKey>,
    /// The script's source text, when it has any.
    pub script_source: Option<String>,
}

/// Why a placed reference is pointed at; see [`Records::referenced_refs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefReason {
    /// A quest objective target (`QSTA`).
    QuestTarget,
    /// A script's reference list (`SCRO`): of a script record, a quest
    /// stage, a dialogue result, a terminal item or a package event.
    Script,
    /// The reference a condition runs on (`conditions.reference`).
    Condition,
    /// Another reference's linked reference.
    LinkedRef,
    /// Another reference's enable parent.
    EnableParent,
    /// It has an enable parent of its own.
    EnableChild,
    /// A package's target (`PTDT`, `PTD2`).
    PackageTarget,
    /// A package's location (`PLDT`, `PLD2`).
    PackageLocation,
    /// A reference named by a parameter of a condition
    /// (`conditions.param1_ref`, `param2_ref`): `GetIsReference`,
    /// `GetDistance`, `GetInSameCell` and the other functions of
    /// `fnv_esm::REFERENCE_PARAMETER_FUNCTIONS`. Revision 2.
    ConditionParam,
    /// The destination door of a teleport door (`XTEL`), which `MoveTo`
    /// into another map needs. `by` is the door that teleports. Revision 2.
    TeleportDestination,
}

impl RefReason {
    /// The reasons [`Records::referenced_refs`] counts: every one but
    /// those revision 2 added ([`RefReason::ConditionParam`] and
    /// [`RefReason::TeleportDestination`]), so that a pass that keeps the
    /// referenced references (declutter) keeps the set it always kept.
    pub fn is_legacy(self) -> bool {
        !matches!(self, Self::ConditionParam | Self::TeleportDestination)
    }
}

/// One thing pointing at a placed reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Referral {
    /// The placed reference pointed at.
    pub reference: FormKey,
    /// Why.
    pub reason: RefReason,
    /// The record that points: a quest, script, info, terminal, package
    /// or placed reference (for [`RefReason::EnableChild`], the parent).
    pub by: FormKey,
    /// Where in it: a condition's or script's context, an objective, or
    /// the subrecord.
    pub context: String,
}

/// A quest objective target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestTargetRow {
    /// Objective index.
    pub objective: i64,
    /// Target number within the objective, from 0.
    pub seq: u32,
    /// The target reference.
    pub target: Option<FormKey>,
    /// Target flags, as stored.
    pub flags: u8,
}

pub(crate) const CONDITION_COLUMNS: &str = "context, function, comparison, value, global, param1, param2, \
     run_on, reference, is_or";

pub(crate) fn condition_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConditionRow> {
    Ok(ConditionRow {
        context: r.get(0)?,
        function: r.get::<_, i64>(1)? as u32,
        comparison: r.get(2)?,
        value: r.get(3)?,
        global: form(r.get(4)?),
        param1: r.get::<_, i64>(5)? as u32,
        param2: r.get::<_, i64>(6)? as u32,
        run_on: r.get::<_, i64>(7)? as u32,
        reference: form(r.get(8)?),
        is_or: r.get(9)?,
        param1_ref: form(r.get(10)?),
        param2_ref: form(r.get(11)?),
    })
}

impl Records {
    /// Fails with [`MirError::MissingTable`] when the file lacks a table.
    pub(crate) fn require(&self, table: &'static str) -> Result<(), MirError> {
        let found: Option<i64> = self
            .connection()
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |r| r.get(0),
            )
            .optional()?;
        match found {
            Some(_) => Ok(()),
            None => Err(MirError::MissingTable(table)),
        }
    }

    /// The game the file was built from (`meta.game`); a file written
    /// before it was recorded is New Vegas's.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::Options`] for a value this build
    /// does not know.
    pub fn game(&self) -> Result<fnv_esm::GameVariant, MirError> {
        match self.meta("game")? {
            Some(id) => id.parse().map_err(MirError::Options),
            None => Ok(crate::model::legacy_game()),
        }
    }

    /// Whether a table has a column; the columns revision 2 added are
    /// absent from an older file.
    pub(crate) fn has_column(&self, table: &str, column: &str) -> Result<bool, MirError> {
        let found: Option<i64> = self
            .connection()
            .query_row(
                "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2",
                [table, column],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// `column` when the table has it, else `NULL`.
    pub(crate) fn column_or_null(&self, table: &str, column: &str) -> Result<String, MirError> {
        Ok(if self.has_column(table, column)? {
            column.to_owned()
        } else {
            "NULL".to_owned()
        })
    }

    /// The conditions of a record, in file order.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn conditions(&self, owner: &FormKey) -> Result<Vec<ConditionRow>, MirError> {
        let p1 = self.column_or_null("conditions", "param1_ref")?;
        let p2 = self.column_or_null("conditions", "param2_ref")?;
        let sql = format!(
            "SELECT {CONDITION_COLUMNS}, {p1}, {p2} FROM conditions WHERE owner = ?1 ORDER BY seq"
        );
        let mut statement = self.connection().prepare(&sql)?;
        let rows = statement.query_map([owner.to_string()], condition_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every terminal with its menu, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn terminals(&self) -> Result<Vec<TerminalRow>, MirError> {
        self.terminal_rows(None)
    }

    /// One terminal with its menu.
    ///
    /// # Errors
    ///
    /// As [`Records::terminals`].
    pub fn terminal(&self, key: &FormKey) -> Result<Option<TerminalRow>, MirError> {
        Ok(self.terminal_rows(Some(key))?.into_iter().next())
    }

    fn terminal_rows(&self, only: Option<&FormKey>) -> Result<Vec<TerminalRow>, MirError> {
        self.require("terminals")?;
        let conn = self.connection();
        let filter = if only.is_some() {
            " WHERE form = ?1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT form, editor_id, name, model, script, header_text, difficulty, flags, \
             server_type, password_note, sound FROM terminals{filter} ORDER BY form"
        );
        let mut statement = conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok(TerminalRow {
                form: form(r.get(0)?).unwrap_or_else(|| FormKey::new("", 0)),
                editor_id: r.get(1)?,
                name: r.get(2)?,
                model: r.get(3)?,
                script: form(r.get(4)?),
                header_text: r.get(5)?,
                difficulty: r.get::<_, Option<i64>>(6)?.map(|v| v as u8),
                flags: r.get::<_, Option<i64>>(7)?.map(|v| v as u8),
                server_type: r.get::<_, Option<i64>>(8)?.map(|v| v as u8),
                password_note: form(r.get(9)?),
                sound: form(r.get(10)?),
                items: Vec::new(),
            })
        };
        let mut terminals: Vec<TerminalRow> = match only {
            Some(key) => statement
                .query_map([key.to_string()], map)?
                .collect::<Result<_, _>>()?,
            None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
        };
        let mut items = conn.prepare(
            "SELECT seq, text, result_text, flags, display_note, sub_terminal, script_source \
             FROM terminal_items WHERE terminal = ?1 ORDER BY seq",
        )?;
        for terminal in &mut terminals {
            let all = self.conditions(&terminal.form)?;
            let rows = items.query_map([terminal.form.to_string()], |r| {
                Ok(TerminalItemRow {
                    seq: r.get::<_, i64>(0)? as u32,
                    text: r.get(1)?,
                    result_text: r.get(2)?,
                    flags: r.get::<_, Option<i64>>(3)?.map(|v| v as u8),
                    display_note: form(r.get(4)?),
                    sub_terminal: form(r.get(5)?),
                    script_source: r.get(6)?,
                    conditions: Vec::new(),
                })
            })?;
            for row in rows {
                let mut item = row?;
                let context = format!("item {}", item.seq);
                item.conditions = all
                    .iter()
                    .filter(|c| c.context == context)
                    .cloned()
                    .collect();
                terminal.items.push(item);
            }
        }
        Ok(terminals)
    }

    /// Every note, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn notes(&self) -> Result<Vec<NoteRow>, MirError> {
        self.require("notes")?;
        let conn = self.connection();
        let mut quests: BTreeMap<String, Vec<FormKey>> = BTreeMap::new();
        {
            let mut statement =
                conn.prepare("SELECT note, quest FROM note_quests ORDER BY note, seq")?;
            let rows = statement.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            for row in rows {
                let (note, quest) = row?;
                quests.entry(note).or_default().extend(form(quest));
            }
        }
        let mut statement = conn.prepare(
            "SELECT form, editor_id, name, model, note_type, text, topic, sound, texture \
             FROM notes ORDER BY form",
        )?;
        let rows = statement.query_map([], |r| {
            let key: String = r.get(0)?;
            Ok((
                key.clone(),
                NoteRow {
                    form: form(Some(key)).unwrap_or_else(|| FormKey::new("", 0)),
                    editor_id: r.get(1)?,
                    name: r.get(2)?,
                    model: r.get(3)?,
                    note_type: r.get::<_, Option<i64>>(4)?.map(|v| v as u8),
                    text: r.get(5)?,
                    topic: form(r.get(6)?),
                    sound: form(r.get(7)?),
                    texture: r.get(8)?,
                    quests: Vec::new(),
                },
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (key, mut note) = row?;
            note.quests = quests.remove(&key).unwrap_or_default();
            out.push(note);
        }
        Ok(out)
    }

    /// Every AI package, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn packages(&self) -> Result<Vec<PackageRow>, MirError> {
        self.package_rows(None)
    }

    /// One AI package.
    ///
    /// # Errors
    ///
    /// As [`Records::packages`].
    pub fn package(&self, key: &FormKey) -> Result<Option<PackageRow>, MirError> {
        Ok(self.package_rows(Some(key))?.into_iter().next())
    }

    fn package_rows(&self, only: Option<&FormKey>) -> Result<Vec<PackageRow>, MirError> {
        self.require("packages")?;
        let conn = self.connection();
        let filter = if only.is_some() {
            " WHERE form = ?1"
        } else {
            ""
        };
        let extra = [
            "pkdt_size",
            "behaviour_flags",
            "behaviour_raw",
            "type_flags",
        ]
        .iter()
        .map(|c| self.column_or_null("packages", c))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
        let sql = format!(
            "SELECT form, editor_id, package_type, type_name, flags, month, day_of_week, date, \
             hour, duration, location_type, location, location_value, location_radius, \
             location2_type, location2, location2_value, location2_radius, target_type, target, \
             target_value, target_count, target2_type, target2, target2_value, target2_count, \
             dialogue_topic, combat_style, {extra} FROM packages{filter} ORDER BY form"
        );
        let mut statement = conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            let int = |i: usize| r.get::<_, Option<i64>>(i);
            let schedule = match (int(5)?, int(6)?, int(7)?, int(8)?, int(9)?) {
                (Some(month), Some(day), Some(date), Some(hour), Some(duration)) => {
                    Some(fnv_esm::PackageSchedule {
                        month: month as i8,
                        day_of_week: day as i8,
                        date: date as u8,
                        hour: hour as i8,
                        duration: duration as i32,
                    })
                }
                _ => None,
            };
            let location = |at: usize| -> rusqlite::Result<Option<PackageLocationRow>> {
                Ok(match int(at)? {
                    Some(kind) => Some(PackageLocationRow {
                        location_type: kind as i32,
                        form: form(r.get(at + 1)?),
                        value: int(at + 2)?.unwrap_or(0) as u32,
                        radius: int(at + 3)?.unwrap_or(0) as i32,
                    }),
                    None => None,
                })
            };
            let target = |at: usize| -> rusqlite::Result<Option<PackageTargetRow>> {
                Ok(match int(at)? {
                    Some(kind) => Some(PackageTargetRow {
                        target_type: kind as i32,
                        form: form(r.get(at + 1)?),
                        value: int(at + 2)?.unwrap_or(0) as u32,
                        count: int(at + 3)?.unwrap_or(0) as i32,
                    }),
                    None => None,
                })
            };
            Ok(PackageRow {
                form: form(r.get(0)?).unwrap_or_else(|| FormKey::new("", 0)),
                editor_id: r.get(1)?,
                package_type: int(2)?.map(|v| v as u8),
                type_name: r.get(3)?,
                flags: int(4)?.map(|v| v as u32),
                schedule,
                location: location(10)?,
                location2: location(14)?,
                target: target(18)?,
                target2: target(22)?,
                dialogue_topic: form(r.get(26)?),
                combat_style: form(r.get(27)?),
                conditions: Vec::new(),
                scripts: Vec::new(),
                pkdt_size: int(28)?.map(|v| v as u8),
                behaviour_flags: int(29)?.map(|v| v as u16),
                behaviour_flags_raw: int(30)?.map(|v| v as u16),
                type_flags: int(31)?.map(|v| v as u16),
                events: Vec::new(),
            })
        };
        let mut packages: Vec<PackageRow> = match only {
            Some(key) => statement
                .query_map([key.to_string()], map)?
                .collect::<Result<_, _>>()?,
            None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
        };
        let mut scripts = conn.prepare(
            "SELECT context, source FROM embedded_scripts WHERE owner = ?1 ORDER BY rowid",
        )?;
        let have_events = self.require("package_events").is_ok();
        for package in &mut packages {
            package.conditions = self.conditions(&package.form)?;
            let rows = scripts.query_map([package.form.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            package.scripts = rows.collect::<Result<_, _>>()?;
            if have_events {
                let mut events = conn.prepare(
                    "SELECT event, idle, topic, script_source FROM package_events \
                     WHERE package = ?1 ORDER BY rowid",
                )?;
                let rows = events.query_map([package.form.to_string()], |r| {
                    Ok(PackageEventRow {
                        event: r.get(0)?,
                        idle: form(r.get(1)?),
                        topic: form(r.get(2)?),
                        script_source: r.get(3)?,
                    })
                })?;
                package.events = rows.collect::<Result<_, _>>()?;
            }
        }
        Ok(packages)
    }

    /// The AI packages of an NPC or creature (`PKID`), in file order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn actor_packages(&self, actor: &FormKey) -> Result<Vec<FormKey>, MirError> {
        self.require("actor_packages")?;
        let mut statement = self
            .connection()
            .prepare("SELECT package FROM actor_packages WHERE actor = ?1 ORDER BY seq")?;
        let rows = statement.query_map([actor.to_string()], |r| r.get::<_, Option<String>>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.extend(form(row?));
        }
        Ok(out)
    }

    /// The NPCs and creatures that list a package, sorted.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn package_actors(&self, package: &FormKey) -> Result<Vec<FormKey>, MirError> {
        self.require("actor_packages")?;
        let mut statement = self
            .connection()
            .prepare("SELECT DISTINCT actor FROM actor_packages WHERE package = ?1")?;
        let rows = statement.query_map([package.to_string()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.extend(form(Some(row?)));
        }
        out.sort();
        Ok(out)
    }

    /// The targets of a quest's objectives, in objective and target order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn quest_targets(&self, quest: &FormKey) -> Result<Vec<QuestTargetRow>, MirError> {
        self.require("quest_targets")?;
        let mut statement = self.connection().prepare(
            "SELECT objective, seq, target, flags FROM quest_targets WHERE quest = ?1 \
             ORDER BY objective, seq",
        )?;
        let rows = statement.query_map([quest.to_string()], |r| {
            Ok(QuestTargetRow {
                objective: r.get(0)?,
                seq: r.get::<_, i64>(1)? as u32,
                target: form(r.get(2)?),
                flags: r.get::<_, i64>(3)? as u8,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The forms a record's scripts name (`SCRO`), with the context of
    /// each script: `"script"` for a script record, `"stage 10 entry 0"`,
    /// `"result begin"`, `"item 2"`, `"on begin"` and so on.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn script_references(&self, owner: &FormKey) -> Result<Vec<(String, FormKey)>, MirError> {
        self.require("script_refs")?;
        let mut statement = self
            .connection()
            .prepare("SELECT context, form FROM script_refs WHERE owner = ?1 ORDER BY rowid")?;
        let rows = statement.query_map([owner.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (context, target) = row?;
            if let Some(target) = form(Some(target)) {
                out.push((context, target));
            }
        }
        Ok(out)
    }

    /// Everything that points at a placed reference, over the whole plugin
    /// list, sorted: quest objective targets, script reference lists,
    /// conditions run on a reference, linked references, enable parents
    /// (both sides), package targets and locations, condition parameters
    /// that name a reference and teleport destinations. Only forms that are
    /// placed references (`placed_refs`) are listed.
    ///
    /// Cost: thirteen SQL queries, each one pass over its source table
    /// (`quest_targets`, `script_refs`, `conditions` three times,
    /// `placed_refs` four times, `packages` four times, once per column)
    /// with a primary-key lookup in `placed_refs` per row, then a sort;
    /// memory is one
    /// [`Referral`] per row found. Measured on the retail masters: 0.25 s
    /// for Fallout 3 (573,610 placed references, 49,448 conditions, 19,362
    /// script references; 14,274 references found) and 0.29 s for New
    /// Vegas (314,095 placed references; 12,255 found).
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn ref_referrals(&self) -> Result<Vec<Referral>, MirError> {
        for table in ["quest_targets", "script_refs", "packages"] {
            self.require(table)?;
        }
        let new_tables = self.has_column("conditions", "param1_ref")?;
        let queries: [(RefReason, &str); 13] = [
            (
                RefReason::QuestTarget,
                "SELECT q.target, q.quest, 'objective ' || q.objective FROM quest_targets q \
                 JOIN placed_refs p ON p.form = q.target",
            ),
            (
                RefReason::Script,
                "SELECT s.form, s.owner, s.context FROM script_refs s \
                 JOIN placed_refs p ON p.form = s.form",
            ),
            (
                RefReason::Condition,
                "SELECT c.reference, c.owner, c.context FROM conditions c \
                 JOIN placed_refs p ON p.form = c.reference",
            ),
            (
                RefReason::LinkedRef,
                "SELECT r.linked_ref, r.form, 'linked ref' FROM placed_refs r \
                 JOIN placed_refs p ON p.form = r.linked_ref",
            ),
            (
                RefReason::EnableParent,
                "SELECT r.enable_parent, r.form, 'enable parent' FROM placed_refs r \
                 JOIN placed_refs p ON p.form = r.enable_parent",
            ),
            (
                RefReason::EnableChild,
                "SELECT r.form, r.enable_parent, 'enable parent' FROM placed_refs r \
                 WHERE r.enable_parent IS NOT NULL",
            ),
            (
                RefReason::PackageTarget,
                "SELECT k.target, k.form, 'target' FROM packages k \
                 JOIN placed_refs p ON p.form = k.target",
            ),
            (
                RefReason::PackageTarget,
                "SELECT k.target2, k.form, 'target 2' FROM packages k \
                 JOIN placed_refs p ON p.form = k.target2",
            ),
            (
                RefReason::PackageLocation,
                "SELECT k.location, k.form, 'location' FROM packages k \
                 JOIN placed_refs p ON p.form = k.location",
            ),
            (
                RefReason::PackageLocation,
                "SELECT k.location2, k.form, 'location 2' FROM packages k \
                 JOIN placed_refs p ON p.form = k.location2",
            ),
            (
                RefReason::ConditionParam,
                "SELECT c.param1_ref, c.owner, c.context || ' parameter 1' FROM conditions c \
                 JOIN placed_refs p ON p.form = c.param1_ref",
            ),
            (
                RefReason::ConditionParam,
                "SELECT c.param2_ref, c.owner, c.context || ' parameter 2' FROM conditions c \
                 JOIN placed_refs p ON p.form = c.param2_ref",
            ),
            (
                RefReason::TeleportDestination,
                "SELECT r.teleport_door, r.form, 'teleport destination' FROM placed_refs r \
                 JOIN placed_refs p ON p.form = r.teleport_door",
            ),
        ];
        let mut out = Vec::new();
        for (reason, sql) in queries {
            // The two reasons of revision 2 need its columns; an older
            // file has no such references to find.
            if !new_tables && !reason.is_legacy() && sql.contains("param") {
                continue;
            }
            let mut statement = self.connection().prepare(sql)?;
            let rows = statement.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (reference, by, context) = row?;
                if let (Some(reference), Some(by)) = (form(Some(reference)), form(Some(by))) {
                    out.push(Referral {
                        reference,
                        reason,
                        by,
                        context,
                    });
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// For each placed reference something points at, why
    /// ([`Records::ref_referrals`], grouped).
    ///
    /// # Errors
    ///
    /// As [`Records::ref_referrals`].
    pub fn referenced_ref_reasons(
        &self,
    ) -> Result<BTreeMap<FormKey, BTreeSet<RefReason>>, MirError> {
        let mut out: BTreeMap<FormKey, BTreeSet<RefReason>> = BTreeMap::new();
        for referral in self.ref_referrals()? {
            out.entry(referral.reference)
                .or_default()
                .insert(referral.reason);
        }
        Ok(out)
    }

    /// The placed references something points at, for a declutter pass
    /// that must keep them: see [`Records::ref_referrals`]. Only the
    /// reasons of [`RefReason::is_legacy`] count, which is the set this
    /// method has always returned; [`Records::referenced_refs_all`] adds
    /// condition parameters and teleport destinations.
    ///
    /// # Errors
    ///
    /// As [`Records::ref_referrals`].
    pub fn referenced_refs(&self) -> Result<BTreeSet<FormKey>, MirError> {
        Ok(self
            .ref_referrals()?
            .into_iter()
            .filter(|r| r.reason.is_legacy())
            .map(|r| r.reference)
            .collect())
    }

    /// Every placed reference something points at, whatever the reason
    /// ([`RefReason`], revision 2's two included).
    ///
    /// # Errors
    ///
    /// As [`Records::ref_referrals`].
    pub fn referenced_refs_all(&self) -> Result<BTreeSet<FormKey>, MirError> {
        Ok(self
            .ref_referrals()?
            .into_iter()
            .map(|r| r.reference)
            .collect())
    }
}
