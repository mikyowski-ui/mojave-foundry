//! Typed queries over the tables revision 2 of `records.sqlite` added for
//! the simulation layer: globals, game settings, messages, factions, the
//! AI data and factions of actors (resolved through templates), dialogue
//! infos with their links and flags, the whole reference and variable
//! lists of embedded scripts, and the counts of what a decoder could not
//! use.
//!
//! On a file written before revision 2 every method here returns
//! [`MirError::MissingTable`]; extracting again rebuilds the file.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use fnv_esm::{AiData, CombatReaction, GameSettingValue, GlobalType, InfoData, Message};

use crate::error::MirError;
use crate::logic::{ConditionRow, form};
use crate::paths::FormKey;
use crate::reader::Records;

fn key_or_empty(text: Option<String>) -> FormKey {
    form(text).unwrap_or_else(|| FormKey::new("", 0))
}

// ---------------------------------------------------------------- globals

/// A global variable (`GLOB`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalRow {
    /// The global's record.
    pub form: FormKey,
    /// Editor id: the name scripts use.
    pub editor_id: Option<String>,
    /// The type (`FNAM`): short, long or float.
    pub kind: Option<GlobalType>,
    /// The initial value (`FLTV`), whatever the type.
    pub value: Option<f32>,
}

// -------------------------------------------------------------- settings

/// A game setting (`GMST`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSettingRow {
    /// The setting's record.
    pub form: FormKey,
    /// The setting's name (its editor id): `fSpeechChallengeMultiplier`.
    pub name: String,
    /// The value, typed by the name's first letter; `None` for a setting
    /// whose `DATA` did not read.
    pub value: Option<GameSettingValue>,
}

// -------------------------------------------------------------- messages

/// One button of a message box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageButtonRow {
    /// Position, from 0.
    pub seq: u32,
    /// The button's text.
    pub text: Option<String>,
    /// The conditions that show it.
    pub conditions: Vec<ConditionRow>,
}

/// A message (`MESG`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRow {
    /// The message's record.
    pub form: FormKey,
    /// Editor id.
    pub editor_id: Option<String>,
    /// The title (`FULL`).
    pub title: Option<String>,
    /// The text (`DESC`).
    pub text: Option<String>,
    /// The icon.
    pub icon: Option<FormKey>,
    /// Flags (`DNAM`), as stored.
    pub flags: Option<u32>,
    /// The display time in seconds of a notice.
    pub display_time: Option<u32>,
    /// The buttons, in order.
    pub buttons: Vec<MessageButtonRow>,
}

impl MessageRow {
    /// Whether the message is a message box (waits for a button).
    pub fn is_message_box(&self) -> bool {
        self.flags.is_some_and(|f| f & Message::MESSAGE_BOX != 0)
    }

    /// Whether the message is shown without being asked for.
    pub fn is_auto_display(&self) -> bool {
        self.flags.is_some_and(|f| f & Message::AUTO_DISPLAY != 0)
    }
}

// -------------------------------------------------------------- factions

/// One relation of a faction (`XNAM`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactionRelationRow {
    /// The other faction.
    pub other: Option<FormKey>,
    /// The disposition modifier.
    pub modifier: i32,
    /// The group combat reaction as stored; `None` in the older 8-byte
    /// form.
    pub combat_reaction: Option<u32>,
}

impl FactionRelationRow {
    /// The group combat reaction, when stored.
    pub fn reaction(&self) -> Option<CombatReaction> {
        self.combat_reaction.map(CombatReaction::from_code)
    }
}

/// A faction (`FACT`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactionRow {
    /// The faction's record.
    pub form: FormKey,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// The first flag byte (see `fnv_esm::FactionData`).
    pub flags: Option<u8>,
    /// The second flag byte; `None` in the 1-byte `DATA`.
    pub flags2: Option<u8>,
    /// Relations to other factions, in order.
    pub relations: Vec<FactionRelationRow>,
}

impl FactionRow {
    /// The group combat reaction towards `other`, when this faction has a
    /// relation to it that stores one.
    pub fn reaction_towards(&self, other: &FormKey) -> Option<CombatReaction> {
        self.relations
            .iter()
            .find(|r| r.other.as_ref() == Some(other))
            .and_then(FactionRelationRow::reaction)
    }

    /// Whether the faction is hidden from the player.
    pub fn is_hidden_from_pc(&self) -> bool {
        self.flags
            .is_some_and(|f| f & fnv_esm::FactionData::HIDDEN_FROM_PC != 0)
    }

    /// Whether the faction is evil.
    pub fn is_evil(&self) -> bool {
        self.flags
            .is_some_and(|f| f & fnv_esm::FactionData::EVIL != 0)
    }

    /// Whether crimes against the faction are tracked.
    pub fn tracks_crime(&self) -> bool {
        self.flags2
            .is_some_and(|f| f & fnv_esm::FactionData::TRACK_CRIME != 0)
    }
}

// ---------------------------------------------------------------- actors

/// The AI data and voice type of an NPC or creature, read from the actor
/// whose template flags say they come from the template (`fnv_esm::AiData`
/// documents the layout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorAiRow {
    /// The NPC or creature record.
    pub actor: FormKey,
    /// `"npc"` or `"creature"`.
    pub kind: String,
    /// The AI data (`AIDT`); `None` when neither the actor nor the
    /// template that supplies it has one.
    pub ai: Option<AiData>,
    /// The actor the AI data was read from: the actor itself, or a
    /// template reached through its AI-data flag (`0x0010`).
    pub ai_from: Option<FormKey>,
    /// The voice type (`VTCK`).
    pub voice: Option<FormKey>,
    /// The actor the voice type was read from: the actor itself, or a
    /// template reached through its traits flag (`0x0001`).
    pub voice_from: Option<FormKey>,
}

/// One faction an actor belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorFactionRow {
    /// The faction; `None` when it is outside the plugin list.
    pub faction: Option<FormKey>,
    /// The rank in it.
    pub rank: u8,
    /// The actor the membership was read from: the actor itself, or a
    /// template reached through its factions flag (`0x0004`).
    pub from: FormKey,
}

// ----------------------------------------------------------------- infos

/// One response of an info.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfoResponseRow {
    /// Position, from 0.
    pub seq: u32,
    /// The response number, from 1.
    pub number: Option<u8>,
    /// The emotion type, as stored.
    pub emotion_type: Option<u32>,
    /// The emotion value.
    pub emotion_value: Option<i32>,
    /// The spoken text.
    pub text: Option<String>,
}

/// A dialogue info (`INFO`) with its flags and links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InfoRow {
    /// The info's record.
    pub form: FormKey,
    /// The topic it belongs to.
    pub topic: Option<FormKey>,
    /// The quest it belongs to.
    pub quest: Option<FormKey>,
    /// The dialogue type, as stored.
    pub info_type: Option<u8>,
    /// Who speaks next, as stored.
    pub next_speaker: Option<u8>,
    /// The flag word as stored (the second byte is only meaningful when
    /// [`Self::flag_bytes`] is 2).
    pub flags: Option<u16>,
    /// How many flag bytes `DATA` stored: 0, 1 (the older 3-byte form) or
    /// 2.
    pub flag_bytes: u8,
    /// The speaker (`ANAM`).
    pub speaker: Option<FormKey>,
    /// The previous info (`PNAM`).
    pub previous_info: Option<FormKey>,
    /// The player's prompt.
    pub prompt: Option<String>,
    /// The label of a skill check (`KNAM`): an actor value or a perk.
    pub knam: Option<FormKey>,
    /// The signature of the record `KNAM` names (`AVIF` or `PERK`).
    pub knam_sig: Option<String>,
    /// The speech-challenge level (`DNAM`), as stored; see
    /// `fnv_esm::speech_difficulty_name`.
    pub speech_challenge: Option<u32>,
    /// The responses, in order.
    pub responses: Vec<InfoResponseRow>,
    /// Topics offered next (`TCLT`), in order.
    pub choices: Vec<FormKey>,
    /// Topics that link here (`TCLF`), in order.
    pub link_from: Vec<FormKey>,
    /// Follow-up topics (`TCFU`), in order.
    pub follow_up: Vec<FormKey>,
    /// Topics this info adds (`NAME`), in order.
    pub add_topics: Vec<FormKey>,
    /// The conditions, in order.
    pub conditions: Vec<ConditionRow>,
}

impl InfoRow {
    /// The `DATA` as `fnv_esm` reads it, for its named flag accessors
    /// (`is_goodbye`, `is_speech_challenge`, `is_say_once_a_day`, ...).
    pub fn data(&self) -> InfoData {
        InfoData {
            info_type: self.info_type.unwrap_or(0),
            next_speaker: self.next_speaker.unwrap_or(0),
            flags: if self.flag_bytes == 0 {
                None
            } else {
                self.flags
            },
            short: self.flag_bytes == 1,
        }
    }
}

// --------------------------------------------------------------- scripts

/// One row of a script's reference list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptRefRow {
    /// The 1-based position compiled code uses to refer to this entry.
    pub index: u32,
    /// The resolved form, for a form entry that is in the plugin list.
    pub resolved: Option<FormKey>,
    /// What the entry is.
    pub entry: ScriptRefKind,
}

/// The two kinds of reference-list entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptRefKind {
    /// A form (`SCRO`), with the form id as stored.
    Form(u32),
    /// A local variable (`SCRV`), by its index.
    Local(u32),
}

impl ScriptRefRow {
    /// The form of a form entry, when it resolves.
    pub fn form(&self) -> Option<&FormKey> {
        self.resolved.as_ref()
    }
}

/// A local variable of a script (`SLSD` and `SCVR`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptLocalRow {
    /// The variable's index.
    pub index: u32,
    /// Its name; `None` when the script stored none.
    pub name: Option<String>,
    /// Flags, as stored; bit 0 marks an integer variable.
    pub flags: u8,
}

impl ScriptLocalRow {
    /// Whether the variable is an integer (`short` or `long`), not a
    /// float or a reference.
    pub fn is_integer(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// One embedded script body with its owner and context: its header, whole
/// reference list and variable declarations, so that a compiler resolves
/// the names in the source through the script's own lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptContextRow {
    /// The record that holds the script: a script, quest, info, terminal
    /// or package.
    pub owner: FormKey,
    /// Where in the owner: `"script"`, `"stage 10 entry 0"`, `"result
    /// begin"`, `"result end"`, `"item 2"`, `"on begin"`, `"on end"` or
    /// `"on change"`.
    pub context: String,
    /// The script type of the header: 0 object, 1 quest, 0x100 effect.
    pub script_type: Option<u16>,
    /// The header flags.
    pub flags: Option<u16>,
    /// The reference count the header states.
    pub ref_count: Option<u32>,
    /// The compiled size the header states.
    pub compiled_size: Option<u32>,
    /// The variable count the header states.
    pub variable_count: Option<u32>,
    /// Whether the script has source text.
    pub has_source: bool,
    /// Whether the script has compiled bytes.
    pub has_compiled: bool,
    /// The source text, when it has any.
    pub source: Option<String>,
    /// The reference list, in order.
    pub references: Vec<ScriptRefRow>,
    /// The variable declarations, in order.
    pub locals: Vec<ScriptLocalRow>,
}

impl ScriptContextRow {
    /// A variable by name (case does not matter).
    pub fn local(&self, name: &str) -> Option<&ScriptLocalRow> {
        self.locals.iter().find(|l| {
            l.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
    }

    /// The form a 1-based reference index names; `None` for a local entry,
    /// an unresolved form or an index outside the list.
    pub fn form_at(&self, index: u32) -> Option<&FormKey> {
        self.references
            .iter()
            .find(|r| r.index == index)
            .and_then(ScriptRefRow::form)
    }
}

// ----------------------------------------------------------------- skips

/// What the decoders could not use, counted over the whole plugin list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeSkipRow {
    /// The record and subrecord: `"PACK/PKDT"`, `"CTDA/parameter"`.
    pub area: String,
    /// Why: `"not decoded"`, `"malformed, repeated or out of place, size
    /// 10"`, or a named absence such as `"8-byte form has no
    /// type-specific flags"`.
    pub reason: String,
    /// How many records (or conditions).
    pub count: u64,
}

impl Records {
    // ------------------------------------------------------------ globals

    /// Every global variable, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn globals(&self) -> Result<Vec<GlobalRow>, MirError> {
        self.global_rows("", None)
    }

    /// One global variable by record.
    ///
    /// # Errors
    ///
    /// As [`Records::globals`].
    pub fn global(&self, key: &FormKey) -> Result<Option<GlobalRow>, MirError> {
        let text = key.to_string();
        Ok(self.global_rows(" WHERE form = ?1", Some(&text))?.pop())
    }

    /// One global variable by editor id (case does not matter).
    ///
    /// # Errors
    ///
    /// As [`Records::globals`].
    pub fn global_by_name(&self, editor_id: &str) -> Result<Option<GlobalRow>, MirError> {
        Ok(self
            .global_rows(" WHERE editor_id = ?1 COLLATE NOCASE", Some(editor_id))?
            .into_iter()
            .next())
    }

    fn global_rows(&self, filter: &str, param: Option<&str>) -> Result<Vec<GlobalRow>, MirError> {
        self.require("globals")?;
        let sql =
            format!("SELECT form, editor_id, type_code, value FROM globals{filter} ORDER BY form");
        let mut statement = self.connection().prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            let code: Option<String> = r.get(2)?;
            Ok(GlobalRow {
                form: key_or_empty(r.get(0)?),
                editor_id: r.get(1)?,
                kind: code
                    .and_then(|c| c.bytes().next())
                    .map(GlobalType::from_code),
                value: r.get::<_, Option<f64>>(3)?.map(|v| v as f32),
            })
        };
        let rows = match param {
            Some(p) => statement.query_map([p], map)?.collect::<Result<_, _>>()?,
            None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
        };
        Ok(rows)
    }

    // ----------------------------------------------------------- settings

    /// Every game setting, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn game_settings(&self) -> Result<Vec<GameSettingRow>, MirError> {
        self.setting_rows("", None)
    }

    /// One game setting by name (case does not matter).
    ///
    /// # Errors
    ///
    /// As [`Records::game_settings`].
    pub fn game_setting(&self, name: &str) -> Result<Option<GameSettingRow>, MirError> {
        Ok(self
            .setting_rows(" WHERE editor_id = ?1 COLLATE NOCASE", Some(name))?
            .into_iter()
            .next())
    }

    /// A numeric game setting as a float; `None` when there is no such
    /// setting or it is a string.
    ///
    /// # Errors
    ///
    /// As [`Records::game_settings`].
    pub fn game_setting_f32(&self, name: &str) -> Result<Option<f32>, MirError> {
        Ok(self
            .game_setting(name)?
            .and_then(|s| s.value)
            .and_then(|v| v.as_f32()))
    }

    /// An integer game setting; `None` when there is no such setting or it
    /// is not an integer kind.
    ///
    /// # Errors
    ///
    /// As [`Records::game_settings`].
    pub fn game_setting_i64(&self, name: &str) -> Result<Option<i64>, MirError> {
        Ok(self
            .game_setting(name)?
            .and_then(|s| s.value)
            .and_then(|v| v.as_i64()))
    }

    fn setting_rows(
        &self,
        filter: &str,
        param: Option<&str>,
    ) -> Result<Vec<GameSettingRow>, MirError> {
        self.require("game_settings")?;
        let sql = format!(
            "SELECT form, editor_id, kind, int_value, float_value, text_value \
             FROM game_settings{filter} ORDER BY form"
        );
        let mut statement = self.connection().prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            let kind: Option<String> = r.get(2)?;
            let int: Option<i64> = r.get(3)?;
            let float: Option<f64> = r.get(4)?;
            let text: Option<String> = r.get(5)?;
            let value = match (kind.as_deref(), int, float, text) {
                (Some("int"), Some(v), _, _) => Some(GameSettingValue::Int(v as i32)),
                (Some("unsigned"), Some(v), _, _) => Some(GameSettingValue::Unsigned(v as u32)),
                (Some("bool"), Some(v), _, _) => Some(GameSettingValue::Bool(v != 0)),
                (Some("float"), _, Some(v), _) => Some(GameSettingValue::Float(v as f32)),
                (Some("string"), _, _, Some(v)) => Some(GameSettingValue::String(v)),
                _ => None,
            };
            Ok(GameSettingRow {
                form: key_or_empty(r.get(0)?),
                name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                value,
            })
        };
        let rows = match param {
            Some(p) => statement.query_map([p], map)?.collect::<Result<_, _>>()?,
            None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
        };
        Ok(rows)
    }

    // ----------------------------------------------------------- messages

    /// Every message with its buttons, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn messages(&self) -> Result<Vec<MessageRow>, MirError> {
        self.message_rows(None)
    }

    /// One message with its buttons.
    ///
    /// # Errors
    ///
    /// As [`Records::messages`].
    pub fn message(&self, key: &FormKey) -> Result<Option<MessageRow>, MirError> {
        Ok(self.message_rows(Some(key))?.into_iter().next())
    }

    fn message_rows(&self, only: Option<&FormKey>) -> Result<Vec<MessageRow>, MirError> {
        self.require("messages")?;
        let filter = if only.is_some() {
            " WHERE form = ?1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT form, editor_id, title, text, icon, flags, display_time FROM messages{filter} \
             ORDER BY form"
        );
        let conn = self.connection();
        let mut statement = conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok(MessageRow {
                form: key_or_empty(r.get(0)?),
                editor_id: r.get(1)?,
                title: r.get(2)?,
                text: r.get(3)?,
                icon: form(r.get(4)?),
                flags: r.get::<_, Option<i64>>(5)?.map(|v| v as u32),
                display_time: r.get::<_, Option<i64>>(6)?.map(|v| v as u32),
                buttons: Vec::new(),
            })
        };
        let mut messages: Vec<MessageRow> = match only {
            Some(key) => statement
                .query_map([key.to_string()], map)?
                .collect::<Result<_, _>>()?,
            None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
        };
        let mut buttons =
            conn.prepare("SELECT seq, text FROM message_buttons WHERE message = ?1 ORDER BY seq")?;
        for message in &mut messages {
            let all = self.conditions(&message.form)?;
            let rows = buttons.query_map([message.form.to_string()], |r| {
                Ok((r.get::<_, i64>(0)? as u32, r.get::<_, Option<String>>(1)?))
            })?;
            for row in rows {
                let (seq, text) = row?;
                let context = format!("button {seq}");
                message.buttons.push(MessageButtonRow {
                    seq,
                    text,
                    conditions: all
                        .iter()
                        .filter(|c| c.context == context)
                        .cloned()
                        .collect(),
                });
            }
        }
        Ok(messages)
    }

    // ----------------------------------------------------------- factions

    /// Every faction with its relations, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn factions(&self) -> Result<Vec<FactionRow>, MirError> {
        self.faction_rows(None, None)
    }

    /// One faction with its relations.
    ///
    /// # Errors
    ///
    /// As [`Records::factions`].
    pub fn faction(&self, key: &FormKey) -> Result<Option<FactionRow>, MirError> {
        let text = key.to_string();
        Ok(self
            .faction_rows(Some(" WHERE form = ?1"), Some(&text))?
            .into_iter()
            .next())
    }

    /// One faction by editor id (case does not matter): `PlayerFaction`.
    ///
    /// # Errors
    ///
    /// As [`Records::factions`].
    pub fn faction_by_name(&self, editor_id: &str) -> Result<Option<FactionRow>, MirError> {
        Ok(self
            .faction_rows(
                Some(" WHERE editor_id = ?1 COLLATE NOCASE"),
                Some(editor_id),
            )?
            .into_iter()
            .next())
    }

    fn faction_rows(
        &self,
        filter: Option<&str>,
        param: Option<&str>,
    ) -> Result<Vec<FactionRow>, MirError> {
        self.require("factions")?;
        let sql = format!(
            "SELECT form, editor_id, name, flags, flags2 FROM factions{} ORDER BY form",
            filter.unwrap_or("")
        );
        let conn = self.connection();
        let mut statement = conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok(FactionRow {
                form: key_or_empty(r.get(0)?),
                editor_id: r.get(1)?,
                name: r.get(2)?,
                flags: r.get::<_, Option<i64>>(3)?.map(|v| v as u8),
                flags2: r.get::<_, Option<i64>>(4)?.map(|v| v as u8),
                relations: Vec::new(),
            })
        };
        let mut factions: Vec<FactionRow> = match param {
            Some(p) => statement.query_map([p], map)?.collect::<Result<_, _>>()?,
            None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
        };
        let mut relations = conn.prepare(
            "SELECT other, modifier, reaction FROM faction_relations WHERE faction = ?1 \
             ORDER BY seq",
        )?;
        for faction in &mut factions {
            let rows = relations.query_map([faction.form.to_string()], |r| {
                Ok(FactionRelationRow {
                    other: form(r.get(0)?),
                    modifier: r.get::<_, i64>(1)? as i32,
                    combat_reaction: r.get::<_, Option<i64>>(2)?.map(|v| v as u32),
                })
            })?;
            faction.relations = rows.collect::<Result<_, _>>()?;
        }
        Ok(factions)
    }

    // ------------------------------------------------------------- actors

    /// The AI data and voice type of every NPC and creature, in form
    /// order, each resolved through the actor's templates.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn actor_ais(&self) -> Result<Vec<ActorAiRow>, MirError> {
        self.actor_ai_rows(None)
    }

    /// The AI data and voice type of one NPC or creature, resolved through
    /// its templates.
    ///
    /// # Errors
    ///
    /// As [`Records::actor_ais`].
    pub fn actor_ai(&self, actor: &FormKey) -> Result<Option<ActorAiRow>, MirError> {
        Ok(self.actor_ai_rows(Some(actor))?.into_iter().next())
    }

    fn actor_ai_rows(&self, only: Option<&FormKey>) -> Result<Vec<ActorAiRow>, MirError> {
        self.require("actor_ai")?;
        let filter = if only.is_some() {
            " WHERE actor = ?1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT actor, kind, aggression, confidence, energy_level, responsibility, mood, \
             services, trained_skill, trainer_level, assistance, aggro_radius_behaviour, \
             aggro_radius, ai_from, voice, voice_from FROM actor_ai{filter} ORDER BY actor"
        );
        let mut statement = self.connection().prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            let int = |i: usize| r.get::<_, Option<i64>>(i);
            let ai = match int(2)? {
                Some(aggression) => Some(AiData {
                    aggression: aggression as u8,
                    confidence: int(3)?.unwrap_or(0) as u8,
                    energy_level: int(4)?.unwrap_or(0) as u8,
                    responsibility: int(5)?.unwrap_or(0) as u8,
                    mood: int(6)?.unwrap_or(0) as u8,
                    services: int(7)?.unwrap_or(0) as u32,
                    trained_skill: int(8)?.unwrap_or(0) as i8,
                    trainer_level: int(9)?.unwrap_or(0) as u8,
                    assistance: int(10)?.unwrap_or(0) as u8,
                    aggro_radius_behaviour: int(11)?.unwrap_or(0) as u8,
                    aggro_radius: int(12)?.unwrap_or(0) as i32,
                }),
                None => None,
            };
            Ok(ActorAiRow {
                actor: key_or_empty(r.get(0)?),
                kind: r.get(1)?,
                ai,
                ai_from: form(r.get(13)?),
                voice: form(r.get(14)?),
                voice_from: form(r.get(15)?),
            })
        };
        let rows = match only {
            Some(key) => statement
                .query_map([key.to_string()], map)?
                .collect::<Result<_, _>>()?,
            None => statement.query_map([], map)?.collect::<Result<_, _>>()?,
        };
        Ok(rows)
    }

    /// The factions of an NPC or creature, in order, resolved through its
    /// templates (the factions flag, `0x0004`).
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn actor_factions(&self, actor: &FormKey) -> Result<Vec<ActorFactionRow>, MirError> {
        self.require("actor_factions")?;
        let mut statement = self.connection().prepare(
            "SELECT faction, rank, from_actor FROM actor_factions WHERE actor = ?1 ORDER BY seq",
        )?;
        let rows = statement.query_map([actor.to_string()], |r| {
            Ok(ActorFactionRow {
                faction: form(r.get(0)?),
                rank: r.get::<_, i64>(1)? as u8,
                from: key_or_empty(r.get(2)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // -------------------------------------------------------------- infos

    /// One dialogue info with its flags, responses, links and conditions.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn info(&self, key: &FormKey) -> Result<Option<InfoRow>, MirError> {
        self.require("info_links")?;
        let mut rows = self.info_rows("WHERE form = ?1", key.to_string())?;
        Ok(rows.pop())
    }

    /// The infos of a topic with their flags, responses, links and
    /// conditions, in the order they are stored in the plugin (file
    /// order; [`Records::topic_infos`] is in form order). The order of an
    /// info that a later plugin overrides is that of the plugin that
    /// wins.
    ///
    /// # Errors
    ///
    /// As [`Records::info`].
    pub fn topic_info_rows(&self, topic: &FormKey) -> Result<Vec<InfoRow>, MirError> {
        self.require("info_links")?;
        self.info_rows("WHERE topic = ?1", topic.to_string())
    }

    fn info_rows(&self, filter: &str, param: String) -> Result<Vec<InfoRow>, MirError> {
        let conn = self.connection();
        let sql = format!(
            "SELECT infos.form, topic, quest, info_type, next_speaker, infos.flags, flag_bytes, \
             speaker, previous_info, prompt, knam, knam_sig, speech_challenge FROM infos \
             JOIN records ON records.form = infos.form \
             JOIN plugins ON plugins.name = records.plugin \
             {} ORDER BY plugins.load_order, records.\"offset\"",
            filter.replace("form =", "infos.form =")
        );
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map([param], |r| {
            let int = |i: usize| r.get::<_, Option<i64>>(i);
            Ok(InfoRow {
                form: key_or_empty(r.get(0)?),
                topic: form(r.get(1)?),
                quest: form(r.get(2)?),
                info_type: int(3)?.map(|v| v as u8),
                next_speaker: int(4)?.map(|v| v as u8),
                flags: int(5)?.map(|v| v as u16),
                flag_bytes: int(6)?.unwrap_or(0) as u8,
                speaker: form(r.get(7)?),
                previous_info: form(r.get(8)?),
                prompt: r.get(9)?,
                knam: form(r.get(10)?),
                knam_sig: r.get(11)?,
                speech_challenge: int(12)?.map(|v| v as u32),
                responses: Vec::new(),
                choices: Vec::new(),
                link_from: Vec::new(),
                follow_up: Vec::new(),
                add_topics: Vec::new(),
                conditions: Vec::new(),
            })
        })?;
        let mut infos: Vec<InfoRow> = rows.collect::<Result<_, _>>()?;
        let mut responses = conn.prepare(
            "SELECT seq, number, emotion_type, emotion_value, text FROM info_responses \
             WHERE info = ?1 ORDER BY seq",
        )?;
        let mut links =
            conn.prepare("SELECT kind, topic FROM info_links WHERE info = ?1 ORDER BY kind, seq")?;
        for info in &mut infos {
            let owner = info.form.to_string();
            let rows = responses.query_map([&owner], |r| {
                let int = |i: usize| r.get::<_, Option<i64>>(i);
                Ok(InfoResponseRow {
                    seq: r.get::<_, i64>(0)? as u32,
                    number: int(1)?.map(|v| v as u8),
                    emotion_type: int(2)?.map(|v| v as u32),
                    emotion_value: int(3)?.map(|v| v as i32),
                    text: r.get(4)?,
                })
            })?;
            info.responses = rows.collect::<Result<_, _>>()?;
            let rows = links.query_map([&owner], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            for row in rows {
                let (kind, topic) = row?;
                let Some(topic) = form(topic) else { continue };
                match kind.as_str() {
                    "choice" => info.choices.push(topic),
                    "link_from" => info.link_from.push(topic),
                    "follow_up" => info.follow_up.push(topic),
                    "add_topic" => info.add_topics.push(topic),
                    _ => {}
                }
            }
            info.conditions = self
                .conditions(&info.form)?
                .into_iter()
                .filter(|c| c.context == "info")
                .collect();
        }
        Ok(infos)
    }

    // ------------------------------------------------------------ scripts

    /// Every embedded script body of a record with its header, reference
    /// list and variable declarations, in the order the contexts were
    /// found: a script record gives `"script"`, a quest its stage entries,
    /// an info `"result begin"` and `"result end"`, a terminal its items,
    /// a package its events. A script with no subrecord is left out.
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn script_contexts(&self, owner: &FormKey) -> Result<Vec<ScriptContextRow>, MirError> {
        self.script_context_rows(owner, None)
    }

    /// One embedded script body of a record, by context.
    ///
    /// # Errors
    ///
    /// As [`Records::script_contexts`].
    pub fn script_context(
        &self,
        owner: &FormKey,
        context: &str,
    ) -> Result<Option<ScriptContextRow>, MirError> {
        Ok(self
            .script_context_rows(owner, Some(context))?
            .into_iter()
            .next())
    }

    fn script_context_rows(
        &self,
        owner: &FormKey,
        context: Option<&str>,
    ) -> Result<Vec<ScriptContextRow>, MirError> {
        self.require("script_contexts")?;
        let conn = self.connection();
        let owner_text = owner.to_string();
        let mut statement = conn.prepare(
            "SELECT context, script_type, flags, ref_count, compiled_size, variable_count, \
             has_source, has_compiled FROM script_contexts WHERE owner = ?1 \
             AND (?2 IS NULL OR context = ?2) ORDER BY rowid",
        )?;
        let rows = statement.query_map((&owner_text, context), |r| {
            let int = |i: usize| r.get::<_, Option<i64>>(i);
            Ok(ScriptContextRow {
                owner: owner.clone(),
                context: r.get(0)?,
                script_type: int(1)?.map(|v| v as u16),
                flags: int(2)?.map(|v| v as u16),
                ref_count: int(3)?.map(|v| v as u32),
                compiled_size: int(4)?.map(|v| v as u32),
                variable_count: int(5)?.map(|v| v as u32),
                has_source: r.get(6)?,
                has_compiled: r.get(7)?,
                source: None,
                references: Vec::new(),
                locals: Vec::new(),
            })
        })?;
        let mut contexts: Vec<ScriptContextRow> = rows.collect::<Result<_, _>>()?;
        let mut refs = conn.prepare(
            "SELECT seq, kind, form, raw FROM script_ref_list WHERE owner = ?1 AND context = ?2 \
             ORDER BY seq",
        )?;
        let mut locals = conn.prepare(
            "SELECT idx, name, flags FROM script_locals WHERE owner = ?1 AND context = ?2 \
             ORDER BY rowid",
        )?;
        let mut embedded =
            conn.prepare("SELECT source FROM embedded_scripts WHERE owner = ?1 AND context = ?2")?;
        let mut script_source = conn.prepare("SELECT source FROM scripts WHERE form = ?1")?;
        for script in &mut contexts {
            let rows = refs.query_map((&owner_text, &script.context), |r| {
                let seq = r.get::<_, i64>(0)? as u32;
                let kind: String = r.get(1)?;
                let raw = r.get::<_, i64>(3)? as u32;
                Ok(ScriptRefRow {
                    index: seq + 1,
                    resolved: form(r.get(2)?),
                    entry: if kind == "local" {
                        ScriptRefKind::Local(raw)
                    } else {
                        ScriptRefKind::Form(raw)
                    },
                })
            })?;
            script.references = rows.collect::<Result<_, _>>()?;
            let rows = locals.query_map((&owner_text, &script.context), |r| {
                Ok(ScriptLocalRow {
                    index: r.get::<_, i64>(0)? as u32,
                    name: r.get(1)?,
                    flags: r.get::<_, i64>(2)? as u8,
                })
            })?;
            script.locals = rows.collect::<Result<_, _>>()?;
            script.source = if script.context == "script" {
                script_source
                    .query_row([&owner_text], |r| r.get::<_, Option<String>>(0))
                    .optional()?
                    .flatten()
            } else {
                embedded
                    .query_row((&owner_text, &script.context), |r| r.get::<_, String>(0))
                    .optional()?
            };
        }
        Ok(contexts)
    }

    // -------------------------------------------------------------- skips

    /// What the decoders could not use, counted over the plugin list,
    /// sorted by area and reason: sizes no layout fits, subrecords not
    /// decoded in the simulation records, and named absences (the 8-byte
    /// package form has no type-specific flags, the 3-byte info form has
    /// no second flag byte, a template chain that did not resolve).
    ///
    /// # Errors
    ///
    /// SQLite errors, and [`MirError::MissingTable`] on an older file.
    pub fn decode_skips(&self) -> Result<Vec<DecodeSkipRow>, MirError> {
        self.require("decode_skips")?;
        let mut statement = self
            .connection()
            .prepare("SELECT area, reason, count FROM decode_skips ORDER BY area, reason")?;
        let rows = statement.query_map([], |r| {
            Ok(DecodeSkipRow {
                area: r.get(0)?,
                reason: r.get(1)?,
                count: r.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
