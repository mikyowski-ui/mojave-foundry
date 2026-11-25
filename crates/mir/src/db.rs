//! Building `records.sqlite`: the typed core tables and the lossless raw
//! store, for every record of the plugin list.
//!
//! The file is written in one transaction, rows in form order, indexes
//! last, so that two builds from the same plugins are byte-identical.
//! Records are decoded on `jobs` threads, a wave at a time, while the
//! previous wave is inserted; the insert statements are prepared once.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::thread;

use fnv_esm::{Condition, FormId, Record, Sig, decoder_for, sigs};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use sha2::{Digest, Sha256};

use crate::actors::{USE_AI_DATA, USE_FACTIONS, USE_TRAITS, template_supplier};
use crate::error::MirError;
use crate::index::{BadRecord, Entry, Index, Key, PluginFile, scan_common};
use crate::model::{RECORDS_REVISION, RawStore, SCHEMA_VERSION};
use crate::paths::{creature_model_path, normalize_mesh_path, normalize_texture_path};

macro_rules! tables {
    ($( $variant:ident => $name:literal [ $($column:literal),* $(,)? ] ; )*) => {
        /// The tables, in creation order.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum T { $( $variant ),* }

        /// Name and column definitions of each table, in the order of [`T`].
        pub(crate) const TABLES: &[(&str, &[&str])] = &[ $( ($name, &[ $($column),* ]) ),* ];
    };
}

tables! {
    Records => "records" [
        "id INTEGER PRIMARY KEY", "form TEXT NOT NULL UNIQUE", "sig TEXT NOT NULL",
        "plugin TEXT NOT NULL", "offset INTEGER NOT NULL", "flags INTEGER NOT NULL",
        "compressed INTEGER NOT NULL", "editor_id TEXT", "name TEXT", "model TEXT",
        "script TEXT", "parent TEXT", "bound_min_x INTEGER", "bound_min_y INTEGER",
        "bound_min_z INTEGER", "bound_max_x INTEGER", "bound_max_y INTEGER",
        "bound_max_z INTEGER", "subrecords INTEGER", "decoded INTEGER NOT NULL",
    ];
    Worldspaces => "worldspaces" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "parent TEXT", "flags INTEGER",
        "land_height REAL", "water_height REAL",
    ];
    Cells => "cells" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "worldspace TEXT",
        "x INTEGER", "y INTEGER", "interior INTEGER NOT NULL", "persistent INTEGER NOT NULL",
        "flags INTEGER", "water_height REAL", "owner TEXT",
    ];
    PlacedRefs => "placed_refs" [
        "form TEXT PRIMARY KEY", "kind TEXT NOT NULL", "editor_id TEXT", "base TEXT",
        "base_sig TEXT", "cell TEXT", "worldspace TEXT", "child_group TEXT",
        "x REAL", "y REAL", "z REAL", "rx REAL", "ry REAL", "rz REAL", "scale REAL",
        "persistent INTEGER NOT NULL", "initially_disabled INTEGER NOT NULL",
        "deleted INTEGER NOT NULL", "teleport_door TEXT", "teleport_x REAL", "teleport_y REAL",
        "teleport_z REAL", "teleport_rx REAL", "teleport_ry REAL", "teleport_rz REAL",
        "enable_parent TEXT", "enable_opposite INTEGER", "lock_level INTEGER", "lock_key TEXT",
        "count INTEGER", "linked_ref TEXT", "owner TEXT",
    ];
    Lands => "lands" [
        "form TEXT PRIMARY KEY", "cell TEXT", "has_heights INTEGER NOT NULL",
        "base_layers INTEGER NOT NULL", "alpha_layers INTEGER NOT NULL",
    ];
    Navmeshes => "navmeshes" [
        "form TEXT PRIMARY KEY", "cell TEXT", "vertices INTEGER NOT NULL",
        "triangles INTEGER NOT NULL", "door_links INTEGER NOT NULL",
        "external_links INTEGER NOT NULL",
    ];
    Statics => "statics" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "model TEXT",
    ];
    Doors => "doors" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "flags INTEGER", "open_sound TEXT", "close_sound TEXT",
    ];
    Containers => "containers" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "flags INTEGER", "weight REAL", "items INTEGER NOT NULL", "open_sound TEXT",
        "close_sound TEXT",
    ];
    Inventory => "inventory" [
        "owner TEXT NOT NULL", "seq INTEGER NOT NULL", "item TEXT", "count INTEGER NOT NULL",
    ];
    MiscItems => "misc_items" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "value INTEGER", "weight REAL",
    ];
    Weapons => "weapons" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "value INTEGER", "health INTEGER", "weight REAL", "damage INTEGER", "clip_size INTEGER",
        "ammo TEXT", "animation_type INTEGER", "skill INTEGER", "action_points REAL",
        "min_spread REAL", "spread REAL", "fire_rate REAL", "min_range REAL", "max_range REAL",
        "reload_time REAL", "vats_to_hit INTEGER", "projectiles INTEGER", "reach REAL",
    ];
    Armors => "armors" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "value INTEGER", "health INTEGER", "weight REAL", "armor_rating INTEGER",
        "damage_threshold REAL", "biped_flags INTEGER",
    ];
    Ingestibles => "ingestibles" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "value INTEGER", "weight REAL", "flags INTEGER", "effects INTEGER NOT NULL",
    ];
    Ammo => "ammo" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "value INTEGER", "weight REAL", "speed REAL", "flags INTEGER", "clip_rounds INTEGER",
    ];
    Npcs => "npcs" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "script TEXT", "race TEXT",
        "class TEXT", "female INTEGER", "level INTEGER", "actor_flags INTEGER",
        "template TEXT", "template_flags INTEGER", "voice TEXT", "hair TEXT",
        "hair_length REAL", "hair_color TEXT", "eyes TEXT", "facegen TEXT", "height REAL",
        "weight REAL", "base_health INTEGER", "head_parts INTEGER NOT NULL",
    ];
    Creatures => "creatures" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "creature_type INTEGER", "level INTEGER", "health INTEGER", "damage INTEGER",
        "base_scale REAL", "models INTEGER NOT NULL",
    ];
    LandTextures => "land_textures" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "texture_set TEXT",
    ];
    TextureSets => "texture_sets" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "diffuse TEXT", "normal TEXT",
        "environment_mask TEXT", "glow TEXT", "parallax TEXT", "environment TEXT",
    ];
    Scripts => "scripts" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "script_type INTEGER",
        "variable_count INTEGER", "compiled_size INTEGER", "source TEXT",
    ];
    ScriptVariables => "script_variables" [
        "script TEXT NOT NULL", "idx INTEGER NOT NULL", "name TEXT", "flags INTEGER NOT NULL",
    ];
    EmbeddedScripts => "embedded_scripts" [
        "owner TEXT NOT NULL", "context TEXT NOT NULL", "source TEXT NOT NULL",
    ];
    Quests => "quests" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "script TEXT", "flags INTEGER",
        "priority INTEGER", "stages INTEGER NOT NULL", "objectives INTEGER NOT NULL",
    ];
    QuestStages => "quest_stages" [
        "quest TEXT NOT NULL", "stage INTEGER NOT NULL", "entry INTEGER NOT NULL",
        "flags INTEGER", "text TEXT", "next_quest TEXT",
    ];
    QuestObjectives => "quest_objectives" [
        "quest TEXT NOT NULL", "objective INTEGER NOT NULL", "description TEXT",
        "targets INTEGER NOT NULL",
    ];
    Topics => "topics" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "topic_type INTEGER",
        "flags INTEGER", "priority REAL", "infos INTEGER NOT NULL",
    ];
    TopicQuests => "topic_quests" [
        "topic TEXT NOT NULL", "quest TEXT NOT NULL",
    ];
    Infos => "infos" [
        "form TEXT PRIMARY KEY", "topic TEXT", "quest TEXT", "info_type INTEGER",
        "next_speaker INTEGER", "flags INTEGER", "speaker TEXT", "previous_info TEXT",
        "prompt TEXT", "responses INTEGER NOT NULL", "choices INTEGER NOT NULL",
        "flag_bytes INTEGER", "knam TEXT", "knam_sig TEXT", "speech_challenge INTEGER",
    ];
    InfoResponses => "info_responses" [
        "info TEXT NOT NULL", "seq INTEGER NOT NULL", "number INTEGER", "emotion_type INTEGER",
        "emotion_value INTEGER", "text TEXT",
    ];
    Conditions => "conditions" [
        "owner TEXT NOT NULL", "seq INTEGER NOT NULL", "context TEXT NOT NULL",
        "function INTEGER NOT NULL", "comparison TEXT NOT NULL", "value REAL",
        "global TEXT", "param1 INTEGER NOT NULL", "param2 INTEGER NOT NULL",
        "run_on INTEGER NOT NULL", "reference TEXT", "is_or INTEGER NOT NULL",
        "param1_ref TEXT", "param2_ref TEXT",
    ];
    Terminals => "terminals" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "script TEXT",
        "header_text TEXT", "difficulty INTEGER", "flags INTEGER", "server_type INTEGER",
        "password_note TEXT", "sound TEXT", "items INTEGER NOT NULL",
    ];
    TerminalItems => "terminal_items" [
        "terminal TEXT NOT NULL", "seq INTEGER NOT NULL", "text TEXT", "result_text TEXT",
        "flags INTEGER", "display_note TEXT", "sub_terminal TEXT", "script_source TEXT",
        "conditions INTEGER NOT NULL",
    ];
    Notes => "notes" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "model TEXT", "note_type INTEGER",
        "text TEXT", "topic TEXT", "sound TEXT", "texture TEXT", "quests INTEGER NOT NULL",
    ];
    NoteQuests => "note_quests" [
        "note TEXT NOT NULL", "seq INTEGER NOT NULL", "quest TEXT",
    ];
    Packages => "packages" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "package_type INTEGER", "type_name TEXT",
        "flags INTEGER", "month INTEGER", "day_of_week INTEGER", "date INTEGER", "hour INTEGER",
        "duration INTEGER", "location_type INTEGER", "location TEXT", "location_value INTEGER",
        "location_radius INTEGER", "location2_type INTEGER", "location2 TEXT",
        "location2_value INTEGER", "location2_radius INTEGER", "target_type INTEGER",
        "target TEXT", "target_value INTEGER", "target_count INTEGER", "target2_type INTEGER",
        "target2 TEXT", "target2_value INTEGER", "target2_count INTEGER", "dialogue_topic TEXT",
        "combat_style TEXT", "conditions INTEGER NOT NULL", "pkdt_size INTEGER",
        "behaviour_flags INTEGER", "behaviour_raw INTEGER", "type_flags INTEGER",
    ];
    ActorPackages => "actor_packages" [
        "actor TEXT NOT NULL", "seq INTEGER NOT NULL", "package TEXT",
    ];
    ScriptRefs => "script_refs" [
        "owner TEXT NOT NULL", "context TEXT NOT NULL", "seq INTEGER NOT NULL",
        "form TEXT NOT NULL",
    ];
    QuestTargets => "quest_targets" [
        "quest TEXT NOT NULL", "objective INTEGER NOT NULL", "seq INTEGER NOT NULL",
        "target TEXT", "flags INTEGER NOT NULL",
    ];
    Globals => "globals" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "type_code TEXT", "kind TEXT", "value REAL",
    ];
    GameSettings => "game_settings" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "kind TEXT", "int_value INTEGER",
        "float_value REAL", "text_value TEXT",
    ];
    Messages => "messages" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "title TEXT", "text TEXT", "icon TEXT",
        "flags INTEGER", "display_time INTEGER", "buttons INTEGER NOT NULL",
    ];
    MessageButtons => "message_buttons" [
        "message TEXT NOT NULL", "seq INTEGER NOT NULL", "text TEXT",
        "conditions INTEGER NOT NULL",
    ];
    Factions => "factions" [
        "form TEXT PRIMARY KEY", "editor_id TEXT", "name TEXT", "flags INTEGER", "flags2 INTEGER",
        "relations INTEGER NOT NULL",
    ];
    FactionRelations => "faction_relations" [
        "faction TEXT NOT NULL", "seq INTEGER NOT NULL", "other TEXT", "modifier INTEGER NOT NULL",
        "reaction INTEGER",
    ];
    ActorAi => "actor_ai" [
        "actor TEXT PRIMARY KEY", "kind TEXT NOT NULL", "aggression INTEGER", "confidence INTEGER",
        "energy_level INTEGER", "responsibility INTEGER", "mood INTEGER", "services INTEGER",
        "trained_skill INTEGER", "trainer_level INTEGER", "assistance INTEGER",
        "aggro_radius_behaviour INTEGER", "aggro_radius INTEGER", "ai_from TEXT", "voice TEXT",
        "voice_from TEXT",
    ];
    ActorFactions => "actor_factions" [
        "actor TEXT NOT NULL", "seq INTEGER NOT NULL", "faction TEXT", "rank INTEGER NOT NULL",
        "from_actor TEXT NOT NULL",
    ];
    InfoLinks => "info_links" [
        "info TEXT NOT NULL", "kind TEXT NOT NULL", "seq INTEGER NOT NULL", "topic TEXT",
    ];
    PackageEvents => "package_events" [
        "package TEXT NOT NULL", "event TEXT NOT NULL", "idle TEXT", "topic TEXT",
        "script_source TEXT",
    ];
    ScriptContexts => "script_contexts" [
        "owner TEXT NOT NULL", "context TEXT NOT NULL", "script_type INTEGER", "flags INTEGER",
        "ref_count INTEGER", "compiled_size INTEGER", "variable_count INTEGER",
        "has_source INTEGER NOT NULL", "has_compiled INTEGER NOT NULL",
    ];
    ScriptRefList => "script_ref_list" [
        "owner TEXT NOT NULL", "context TEXT NOT NULL", "seq INTEGER NOT NULL",
        "kind TEXT NOT NULL", "form TEXT", "raw INTEGER NOT NULL",
    ];
    ScriptLocals => "script_locals" [
        "owner TEXT NOT NULL", "context TEXT NOT NULL", "idx INTEGER NOT NULL", "name TEXT",
        "flags INTEGER NOT NULL",
    ];
    DecodeSkips => "decode_skips" [
        "area TEXT NOT NULL", "reason TEXT NOT NULL", "count INTEGER NOT NULL",
    ];
    BadRecords => "bad_records" [
        "seq INTEGER PRIMARY KEY", "plugin TEXT NOT NULL", "offset INTEGER NOT NULL",
        "sig TEXT NOT NULL", "form TEXT", "error TEXT NOT NULL",
    ];
}

/// The lossless raw store, created separately because it is the one table
/// whose rows are not built through [`T`].
const RAW_TABLE: &str = "CREATE TABLE raw_subrecords (record INTEGER NOT NULL, \
     seq INTEGER NOT NULL, sig TEXT NOT NULL, data BLOB NOT NULL)";

/// Indexes, created after the rows.
const INDEXES: &[&str] = &[
    "CREATE INDEX records_sig ON records(sig)",
    "CREATE INDEX records_editor_id ON records(editor_id)",
    "CREATE INDEX raw_subrecords_record ON raw_subrecords(record, seq)",
    "CREATE INDEX cells_grid ON cells(worldspace, x, y)",
    "CREATE INDEX placed_refs_base ON placed_refs(base)",
    "CREATE INDEX placed_refs_cell ON placed_refs(cell)",
    "CREATE INDEX inventory_owner ON inventory(owner, seq)",
    "CREATE INDEX script_variables_script ON script_variables(script, idx)",
    "CREATE INDEX embedded_scripts_owner ON embedded_scripts(owner)",
    "CREATE INDEX quest_stages_quest ON quest_stages(quest, stage, entry)",
    "CREATE INDEX quest_objectives_quest ON quest_objectives(quest, objective)",
    "CREATE INDEX topic_quests_topic ON topic_quests(topic)",
    "CREATE INDEX infos_topic ON infos(topic)",
    "CREATE INDEX info_responses_info ON info_responses(info, seq)",
    "CREATE INDEX conditions_owner ON conditions(owner, seq)",
    "CREATE INDEX conditions_function ON conditions(function)",
    "CREATE INDEX terminal_items_terminal ON terminal_items(terminal, seq)",
    "CREATE INDEX note_quests_note ON note_quests(note, seq)",
    "CREATE INDEX actor_packages_actor ON actor_packages(actor, seq)",
    "CREATE INDEX actor_packages_package ON actor_packages(package)",
    "CREATE INDEX script_refs_owner ON script_refs(owner)",
    "CREATE INDEX quest_targets_quest ON quest_targets(quest, objective, seq)",
    "CREATE INDEX message_buttons_message ON message_buttons(message, seq)",
    "CREATE INDEX faction_relations_faction ON faction_relations(faction, seq)",
    "CREATE INDEX actor_factions_actor ON actor_factions(actor, seq)",
    "CREATE INDEX info_links_info ON info_links(info, kind, seq)",
    "CREATE INDEX package_events_package ON package_events(package)",
    "CREATE INDEX script_contexts_owner ON script_contexts(owner)",
    "CREATE INDEX script_ref_list_owner ON script_ref_list(owner, context, seq)",
    "CREATE INDEX script_locals_owner ON script_locals(owner, context, idx)",
    "CREATE INDEX game_settings_name ON game_settings(editor_id)",
];

/// The form id the player's own reference has in every plugin list; no
/// plugin holds a record for it.
const PLAYER_REFERENCE: u32 = 0x14;

/// Records decoded per thread and wave.
const CHUNK: usize = 2048;

/// What one record contributes to the file.
struct RecordRows {
    rows: Vec<(T, Vec<Value>)>,
    raw: Vec<(Sig, Vec<u8>)>,
    bad: Option<BadRecord>,
    /// What the record's decoders could not use: `(area, reason)`.
    skips: Vec<(String, String)>,
}

/// What a build reports back.
#[derive(Debug, Clone, Default)]
pub(crate) struct BuildStats {
    /// Records written to `records`.
    pub records: usize,
    /// Rows written to `raw_subrecords`.
    pub raw_subrecords: usize,
    /// Rows written to `bad_records`.
    pub bad_records: usize,
}

fn text(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

fn opt_text(s: Option<&str>) -> Value {
    s.map_or(Value::Null, |s| Value::Text(s.to_owned()))
}

fn int(i: impl Into<i64>) -> Value {
    Value::Integer(i.into())
}

fn opt_int<I: Into<i64>>(i: Option<I>) -> Value {
    i.map_or(Value::Null, |i| Value::Integer(i.into()))
}

fn real(f: f32) -> Value {
    if f.is_finite() {
        Value::Real(f64::from(f))
    } else {
        Value::Null
    }
}

fn opt_real(f: Option<f32>) -> Value {
    f.map_or(Value::Null, real)
}

fn flag(b: bool) -> Value {
    Value::Integer(i64::from(b))
}

fn len(n: usize) -> Value {
    Value::Integer(i64::try_from(n).unwrap_or(i64::MAX))
}

/// Resolves form ids stored in one plugin's records.
struct Forms<'i, 'a> {
    index: &'i Index<'a>,
    plugin: u16,
}

impl Forms<'_, '_> {
    fn get(&self, form: FormId) -> Value {
        self.index
            .form(self.plugin, form)
            .map_or(Value::Null, |key| Value::Text(key.to_string()))
    }

    fn opt(&self, form: Option<FormId>) -> Value {
        form.map_or(Value::Null, |f| self.get(f))
    }

    /// The key of the placed reference (`REFR`, `ACHR` or `ACRE`) a form
    /// id names; `None` when it names nothing in the list or a record of
    /// another kind.
    fn placed_ref(&self, form: FormId) -> Option<Value> {
        let key = self.index.resolve(self.plugin, form)?;
        let entry = self.index.get(key)?;
        matches!(entry.sig(), sigs::REFR | sigs::ACHR | sigs::ACRE)
            .then(|| Value::Text(self.index.form_key(key).to_string()))
    }

    /// The signature of the record a form id names, when the list has it.
    fn sig(&self, form: Option<FormId>) -> Value {
        form.and_then(|f| self.index.resolve(self.plugin, f))
            .and_then(|key| self.index.get(key))
            .map_or(Value::Null, |e| Value::Text(e.sig().to_string()))
    }
}

/// The text form of a comparison operator.
pub(crate) fn comparison_text(condition: &Condition) -> String {
    use fnv_esm::Comparison::*;
    match condition.comparison {
        Equal => "==".to_owned(),
        NotEqual => "!=".to_owned(),
        Greater => ">".to_owned(),
        GreaterOrEqual => ">=".to_owned(),
        Less => "<".to_owned(),
        LessOrEqual => "<=".to_owned(),
        Reserved(code) => format!("reserved {code}"),
    }
}

/// Hash of the three FaceGen blobs, each prefixed by its length (or by
/// `u32::MAX` when absent): two NPCs with the same face have the same hash.
fn facegen_hash(blobs: [&Option<Vec<u8>>; 3]) -> Option<String> {
    if blobs.iter().all(|b| b.is_none()) {
        return None;
    }
    let mut hasher = Sha256::new();
    for blob in blobs {
        match blob {
            Some(bytes) => {
                hasher.update((bytes.len() as u32).to_le_bytes());
                hasher.update(bytes);
            }
            None => hasher.update(u32::MAX.to_le_bytes()),
        }
    }
    let digest = hasher.finalize();
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

struct Rows<'f, 'i, 'a> {
    forms: &'f Forms<'i, 'a>,
    owner: String,
    out: Vec<(T, Vec<Value>)>,
    conditions: i64,
    /// Counted notes of what a decoder could not use, as `(area, reason)`;
    /// they end up in `decode_skips`.
    skips: Vec<(String, String)>,
}

impl Rows<'_, '_, '_> {
    fn push(&mut self, table: T, values: Vec<Value>) {
        self.out.push((table, values));
    }

    /// Counts one occurrence of something a decoder could not use.
    fn skip(&mut self, area: &str, reason: impl Into<String>) {
        self.skips.push((area.to_owned(), reason.into()));
    }

    /// The parameters of a condition that name a placed reference,
    /// resolved. A parameter the function can name one with but that does
    /// not resolve to one is counted.
    fn reference_params(&mut self, c: &Condition) -> [Value; 2] {
        let mut out = [Value::Null, Value::Null];
        for (slot, form) in c.reference_params().into_iter().enumerate() {
            let Some(form) = form else { continue };
            match self.forms.placed_ref(form) {
                Some(key) => out[slot] = key,
                None => {
                    let what = if form.0 == PLAYER_REFERENCE {
                        "names the player (form 0x14), which has no record"
                    } else {
                        "is not a placed reference of the plugin list"
                    };
                    self.skip(
                        "CTDA/parameter",
                        format!(
                            "function {} parameter {} {what}",
                            c.function_index(),
                            slot + 1
                        ),
                    );
                }
            }
        }
        out
    }

    fn condition(&mut self, context: &str, c: &Condition) {
        let (value, global) = match c.value {
            fnv_esm::ConditionValue::Float(v) => (real(v), Value::Null),
            fnv_esm::ConditionValue::Global(g) => (Value::Null, self.forms.get(g)),
        };
        let [param1_ref, param2_ref] = self.reference_params(c);
        let row = vec![
            text(self.owner.clone()),
            int(self.conditions),
            text(context),
            int(c.function_index()),
            text(comparison_text(c)),
            value,
            global,
            int(c.param1),
            int(c.param2),
            int(c.run_on),
            if c.reference.is_null() {
                Value::Null
            } else {
                self.forms.get(c.reference)
            },
            flag(c.is_or()),
            param1_ref,
            param2_ref,
        ];
        self.conditions += 1;
        self.push(T::Conditions, row);
    }

    fn inventory(&mut self, items: &[fnv_esm::ContainerItem]) {
        for (seq, item) in items.iter().enumerate() {
            let row = vec![
                text(self.owner.clone()),
                len(seq),
                self.forms.get(item.item),
                int(item.count),
            ];
            self.push(T::Inventory, row);
        }
    }

    fn embedded(&mut self, context: &str, body: &fnv_esm::ScriptBody) {
        if let Some(source) = body.source.as_deref().filter(|s| !s.trim().is_empty()) {
            let row = vec![text(self.owner.clone()), text(context), text(source)];
            self.push(T::EmbeddedScripts, row);
        }
        self.script_refs(context, body);
    }

    /// The forms a script's reference list names (`SCRO`), resolved; a
    /// form outside the plugin list is left out.
    fn script_refs(&mut self, context: &str, body: &fnv_esm::ScriptBody) {
        let forms = body.references.iter().filter_map(|r| match r {
            fnv_esm::ScriptReference::Form(form) => Some(*form),
            fnv_esm::ScriptReference::Local(_) => None,
        });
        for (seq, form) in forms.enumerate() {
            let form = self.forms.get(form);
            if form != Value::Null {
                let row = vec![text(self.owner.clone()), text(context), len(seq), form];
                self.push(T::ScriptRefs, row);
            }
        }
    }

    /// One embedded script body with its header, its whole reference list
    /// (in order, forms and local-variable indexes both: compiled code
    /// indexes it from 1) and its variable declarations.
    fn script_context(&mut self, context: &str, body: &fnv_esm::ScriptBody) {
        let header = body.header;
        let row = vec![
            text(self.owner.clone()),
            text(context),
            opt_int(header.map(|h| h.script_type)),
            opt_int(header.map(|h| h.flags)),
            opt_int(header.map(|h| h.ref_count)),
            opt_int(header.map(|h| h.compiled_size)),
            opt_int(header.map(|h| h.variable_count)),
            flag(body.source.is_some()),
            flag(body.compiled.is_some()),
        ];
        self.push(T::ScriptContexts, row);
        for (seq, reference) in body.references.iter().enumerate() {
            let (kind, form, raw) = match *reference {
                fnv_esm::ScriptReference::Form(form) => ("form", self.forms.get(form), form.0),
                fnv_esm::ScriptReference::Local(index) => ("local", Value::Null, index),
            };
            let row = vec![
                text(self.owner.clone()),
                text(context),
                len(seq),
                text(kind),
                form,
                int(raw),
            ];
            self.push(T::ScriptRefList, row);
        }
        for local in &body.locals {
            let row = vec![
                text(self.owner.clone()),
                text(context),
                int(local.index),
                opt_text(local.name.as_deref()),
                int(local.flags),
            ];
            self.push(T::ScriptLocals, row);
        }
    }

    fn packages(&mut self, packages: &[FormId]) {
        for (seq, package) in packages.iter().enumerate() {
            let row = vec![text(self.owner.clone()), len(seq), self.forms.get(*package)];
            self.push(T::ActorPackages, row);
        }
    }
}

/// The columns of a package location: type, form (for the types whose
/// value is one), raw value, radius.
fn location_values(f: &Forms<'_, '_>, location: Option<fnv_esm::PackageLocation>) -> [Value; 4] {
    match location {
        Some(l) => [
            int(l.location_type),
            f.opt(l.form()),
            int(l.value),
            int(l.radius),
        ],
        None => [Value::Null, Value::Null, Value::Null, Value::Null],
    }
}

/// The columns of a package target: type, form, raw value, count.
fn target_values(f: &Forms<'_, '_>, target: Option<fnv_esm::PackageTarget>) -> [Value; 4] {
    match target {
        Some(t) => [
            int(t.target_type),
            f.opt(t.form()),
            int(t.value),
            int(t.count),
        ],
        None => [Value::Null, Value::Null, Value::Null, Value::Null],
    }
}

/// An actor record a template group is read from.
enum Source<'r> {
    /// The actor being tabled.
    Own(&'r Record),
    /// A template, decoded, with the plugin it was read from.
    Other(u16, Box<Record>),
}

impl Source<'_> {
    fn record(&self) -> &Record {
        match self {
            Self::Own(record) => record,
            Self::Other(_, record) => record,
        }
    }
}

/// The AI data, voice type and factions an actor record holds itself.
type ActorParts<'r> = (
    Option<fnv_esm::AiData>,
    Option<FormId>,
    &'r [fnv_esm::FactionRank],
);

fn actor_parts(record: &Record) -> ActorParts<'_> {
    match record {
        Record::Npc(n) => (n.ai, n.voice, &n.factions),
        Record::Creature(c) => (c.ai, c.voice, &c.factions),
        _ => (None, None, &[]),
    }
}

/// The AI data, voice type and faction rows of an NPC or creature, each
/// group read from the actor its template flag points at, as the actor
/// documents follow templates ([`crate::actors::template_supplier`]):
/// `0x0010` for the AI data, `0x0001` (traits) for the voice type and
/// `0x0004` for the factions. A chain that does not resolve leaves the
/// actor's own data and is counted.
fn actor_tables(index: &Index<'_>, entry: &Entry<'_>, own: &Record, rows: &mut Rows<'_, '_, '_>) {
    let (kind, sig, base, template) = match own {
        Record::Npc(n) => ("npc", "NPC_", n.base, n.template),
        Record::Creature(c) => ("creature", "CREA", c.base, c.template),
        _ => return,
    };
    let flags = base.map_or(0, |b| b.template_flags);
    let has_template = template.is_some_and(|t| !t.is_null());
    let ctx = SupplierContext {
        index,
        entry,
        own,
        sig,
        enabled: has_template,
        flags,
    };
    let source =
        |flag: u16, group: &str, rows: &mut Rows<'_, '_, '_>| ctx.source(flag, group, rows);
    let plugin_of = |source: &Source<'_>| match source {
        Source::Own(_) => entry.plugin,
        Source::Other(plugin, _) => *plugin,
    };
    let key_text = |key: Key| text(index.form_key(key).to_string());

    // AI data.
    let (ai_key, ai_source) = source(USE_AI_DATA, "AI data", rows);
    let ai = ai_source.as_ref().and_then(|s| actor_parts(s.record()).0);
    // Voice type, which the traits group carries.
    let (voice_key, voice_source) = source(USE_TRAITS, "traits", rows);
    let voice_form = voice_source.as_ref().and_then(|s| {
        let plugin = plugin_of(s);
        actor_parts(s.record()).1.map(|v| (plugin, v))
    });
    let voice = voice_form.and_then(|(plugin, voice)| index.form(plugin, voice));
    let mut row = vec![rows_owner(rows), text(kind)];
    match ai {
        Some(a) => row.extend([
            int(a.aggression),
            int(a.confidence),
            int(a.energy_level),
            int(a.responsibility),
            int(a.mood),
            int(a.services),
            int(a.trained_skill),
            int(a.trainer_level),
            int(a.assistance),
            int(a.aggro_radius_behaviour),
            int(a.aggro_radius),
            key_text(ai_key),
        ]),
        None => {
            row.extend(std::iter::repeat_n(Value::Null, 12));
            rows.skip(
                &format!("{sig}/AIDT"),
                "no AIDT on the actor or on the template that supplies its AI data",
            );
        }
    }
    match voice {
        Some(v) => row.extend([text(v.to_string()), key_text(voice_key)]),
        None => {
            row.extend([Value::Null, Value::Null]);
            rows.skip(
                &format!("{sig}/VTCK"),
                "no voice type on the actor or on the template that supplies its traits",
            );
        }
    }
    rows.push(T::ActorAi, row);

    // Factions.
    let (faction_key, faction_source) = source(USE_FACTIONS, "factions", rows);
    if let Some(s) = faction_source {
        let plugin = plugin_of(&s);
        let forms = Forms { index, plugin };
        for (seq, member) in actor_parts(s.record()).2.iter().enumerate() {
            let row = vec![
                rows_owner(rows),
                len(seq),
                forms.get(member.faction),
                int(member.rank),
                key_text(faction_key),
            ];
            rows.push(T::ActorFactions, row);
        }
    }
}

/// What [`actor_tables`] needs to find the actor a group comes from.
struct SupplierContext<'c, 'i, 'a> {
    index: &'c Index<'a>,
    entry: &'c Entry<'i>,
    own: &'c Record,
    sig: &'c str,
    /// The actor names a template.
    enabled: bool,
    /// Its template flags.
    flags: u16,
}

impl<'c> SupplierContext<'c, '_, '_> {
    /// The actor that supplies the group of `flag`, and its record.
    fn source(
        &self,
        flag: u16,
        group: &str,
        rows: &mut Rows<'_, '_, '_>,
    ) -> (Key, Option<Source<'c>>) {
        let key = if self.flags & flag != 0 && self.enabled {
            let (key, notes) = template_supplier(self.index, self.entry.key, flag, group);
            if !notes.is_empty() {
                rows.skip(
                    &format!("{}/TPLT", self.sig),
                    format!(
                        "{group} template chain did not resolve (the actor's own data is used)"
                    ),
                );
            }
            key
        } else {
            if self.flags & flag != 0 {
                rows.skip(
                    &format!("{}/TPLT", self.sig),
                    format!(
                        "{group} flag set but the actor names no template (its own data is used)"
                    ),
                );
            }
            self.entry.key
        };
        let found = if key == self.entry.key {
            Some(Source::Own(self.own))
        } else {
            self.index.get(key).and_then(|e| {
                self.index
                    .decode(e)
                    .ok()
                    .map(|r| Source::Other(e.plugin, Box::new(r)))
            })
        };
        (key, found)
    }
}

fn rows_owner(rows: &Rows<'_, '_, '_>) -> Value {
    text(rows.owner.clone())
}

/// A table and the values of one of its rows.
type TableRow = (T, Vec<Value>);

/// The typed rows of one decoded record, and what it could not use.
fn typed_rows(
    index: &Index<'_>,
    entry: &Entry<'_>,
    record: &Record,
) -> (Vec<TableRow>, Vec<(String, String)>) {
    let forms = Forms {
        index,
        plugin: entry.plugin,
    };
    let owner = index.form_key(entry.key).to_string();
    let mut rows = Rows {
        forms: &forms,
        owner: owner.clone(),
        out: Vec::new(),
        conditions: 0,
        skips: Vec::new(),
    };
    let f = &forms;
    let common = record.common();
    let editor_id = || opt_text(common.and_then(|c| c.editor_id.as_deref()));
    let name = || opt_text(common.and_then(|c| c.full_name.as_deref()));
    let model = || {
        let path = common
            .and_then(|c| c.model.as_deref())
            .map(normalize_mesh_path)
            .filter(|p| !p.is_empty());
        opt_text(path.as_deref())
    };
    let script = || f.opt(common.and_then(|c| c.script));
    let key = || text(owner.clone());

    match record {
        Record::World(w) => {
            let defaults = w.land_defaults;
            rows.push(
                T::Worldspaces,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    f.opt(w.parent),
                    opt_int(w.flags),
                    opt_real(defaults.map(|d| d.land_height)),
                    opt_real(defaults.map(|d| d.water_height)),
                ],
            );
        }
        Record::Cell(c) => {
            let interior = c.is_interior();
            let grid = c.grid.filter(|_| !interior);
            let world = entry.world.map(|k| index.form_key(k).to_string());
            rows.push(
                T::Cells,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    opt_text(world.as_deref()),
                    opt_int(grid.map(|g| g.x)),
                    opt_int(grid.map(|g| g.y)),
                    flag(interior),
                    flag(entry.world.is_some() && !entry.in_block),
                    opt_int(c.flags),
                    opt_real(c.water_height),
                    f.opt(c.owner),
                ],
            );
        }
        Record::Placed(p) => {
            let kind = match p.kind {
                fnv_esm::PlacedKind::Object => "object",
                fnv_esm::PlacedKind::Npc => "npc",
                fnv_esm::PlacedKind::Creature => "creature",
            };
            let group = match entry.child {
                Some(fnv_esm::CellChildKind::Persistent) => "persistent",
                Some(fnv_esm::CellChildKind::Temporary) => "temporary",
                Some(fnv_esm::CellChildKind::VisibleDistant) => "visible_distant",
                None => "none",
            };
            let placement = p.placement.unwrap_or_default();
            let has = p.placement.is_some();
            let pos = |i: usize| {
                if has {
                    real(placement.position[i])
                } else {
                    Value::Null
                }
            };
            let rot = |i: usize| {
                if has {
                    real(placement.rotation[i])
                } else {
                    Value::Null
                }
            };
            let teleport = p.teleport;
            let tele = |v: Option<f32>| opt_real(v);
            let world = entry.world.map(|k| index.form_key(k).to_string());
            let cell = entry.cell.map(|k| index.form_key(k).to_string());
            rows.push(
                T::PlacedRefs,
                vec![
                    key(),
                    text(kind),
                    editor_id(),
                    f.opt(p.base),
                    f.sig(p.base),
                    opt_text(cell.as_deref()),
                    opt_text(world.as_deref()),
                    text(group),
                    pos(0),
                    pos(1),
                    pos(2),
                    rot(0),
                    rot(1),
                    rot(2),
                    real(p.scale),
                    flag(p.is_persistent()),
                    flag(p.is_initially_disabled()),
                    flag(p.is_deleted()),
                    f.opt(teleport.map(|t| t.door)),
                    tele(teleport.map(|t| t.position[0])),
                    tele(teleport.map(|t| t.position[1])),
                    tele(teleport.map(|t| t.position[2])),
                    tele(teleport.map(|t| t.rotation[0])),
                    tele(teleport.map(|t| t.rotation[1])),
                    tele(teleport.map(|t| t.rotation[2])),
                    f.opt(p.enable_parent.map(|e| e.reference)),
                    p.enable_parent
                        .map_or(Value::Null, |e| flag(e.is_opposite())),
                    opt_int(p.lock.map(|l| l.level)),
                    f.opt(p.lock.map(|l| l.key).filter(|k| !k.is_null())),
                    opt_int(p.count),
                    f.opt(p.linked_ref),
                    f.opt(p.owner),
                ],
            );
        }
        Record::Land(l) => {
            let cell = entry.cell.map(|k| index.form_key(k).to_string());
            rows.push(
                T::Lands,
                vec![
                    key(),
                    opt_text(cell.as_deref()),
                    flag(l.heights.is_some()),
                    len(l.base_layers.len()),
                    len(l.alpha_layers.len()),
                ],
            );
        }
        Record::Navmesh(n) => {
            let cell = match n.cell {
                Some(c) => f.get(c),
                None => {
                    let cell = entry.cell.map(|k| index.form_key(k).to_string());
                    opt_text(cell.as_deref())
                }
            };
            rows.push(
                T::Navmeshes,
                vec![
                    key(),
                    cell,
                    len(n.vertices.len()),
                    len(n.triangles.len()),
                    len(n.door_links.len()),
                    len(n.external_links.len()),
                ],
            );
        }
        Record::Static(_) => {
            rows.push(T::Statics, vec![key(), editor_id(), model()]);
        }
        Record::Door(d) => {
            rows.push(
                T::Doors,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    script(),
                    opt_int(d.flags),
                    f.opt(d.open_sound),
                    f.opt(d.close_sound),
                ],
            );
        }
        Record::Container(c) => {
            rows.push(
                T::Containers,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    script(),
                    opt_int(c.data.map(|d| d.flags)),
                    opt_real(c.data.map(|d| d.weight)),
                    len(c.items.len()),
                    f.opt(c.open_sound),
                    f.opt(c.close_sound),
                ],
            );
            rows.inventory(&c.items);
        }
        Record::Misc(m) => {
            rows.push(
                T::MiscItems,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    script(),
                    opt_int(m.data.map(|d| d.value)),
                    opt_real(m.data.map(|d| d.weight)),
                ],
            );
        }
        Record::Weapon(w) => {
            let d = w.data;
            let c = w.combat();
            rows.push(
                T::Weapons,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    script(),
                    opt_int(d.map(|d| d.value)),
                    opt_int(d.map(|d| d.health)),
                    opt_real(d.map(|d| d.weight)),
                    opt_int(d.map(|d| d.damage)),
                    opt_int(d.map(|d| d.clip_size)),
                    f.opt(w.ammo),
                    opt_int(c.map(|c| c.animation_type)),
                    opt_int(c.map(|c| c.skill)),
                    opt_real(c.map(|c| c.action_points)),
                    opt_real(c.map(|c| c.min_spread)),
                    opt_real(c.map(|c| c.spread)),
                    opt_real(c.map(|c| c.fire_rate)),
                    opt_real(c.map(|c| c.min_range)),
                    opt_real(c.map(|c| c.max_range)),
                    opt_real(c.map(|c| c.reload_time)),
                    opt_int(c.map(|c| c.vats_to_hit)),
                    opt_int(c.map(|c| c.projectile_count)),
                    opt_real(c.map(|c| c.reach)),
                ],
            );
        }
        Record::Armor(a) => {
            let d = a.data;
            // An armour's world model is its second model path.
            let world_model = a
                .male_world_model
                .as_deref()
                .map(normalize_mesh_path)
                .filter(|p| !p.is_empty());
            let model = match world_model {
                Some(path) => text(path),
                None => model(),
            };
            rows.push(
                T::Armors,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model,
                    script(),
                    opt_int(d.map(|d| d.value)),
                    opt_int(d.map(|d| d.health)),
                    opt_real(d.map(|d| d.weight)),
                    opt_int(a.stats.map(|s| s.armor_rating)),
                    opt_real(a.stats.and_then(|s| s.damage_threshold)),
                    opt_int(a.biped.map(|b| b.biped_flags)),
                ],
            );
        }
        Record::Ingestible(i) => {
            rows.push(
                T::Ingestibles,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    script(),
                    opt_int(i.data.map(|d| d.value)),
                    opt_real(i.weight),
                    opt_int(i.data.map(|d| d.flags)),
                    len(i.effects.len()),
                ],
            );
            for (number, effect) in i.effects.iter().enumerate() {
                for c in &effect.conditions {
                    rows.condition(&format!("effect {number}"), c);
                }
            }
        }
        Record::Ammo(a) => {
            let d = a.data;
            rows.push(
                T::Ammo,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    script(),
                    opt_int(d.map(|d| d.value)),
                    opt_real(a.data2.map(|d| d.weight)),
                    opt_real(d.map(|d| d.speed)),
                    opt_int(d.map(|d| d.flags)),
                    opt_int(d.map(|d| d.clip_rounds)),
                ],
            );
        }
        Record::Npc(n) => {
            let base = n.base;
            let hair_color = n
                .hair_color
                .map(|c| format!("{:02x}{:02x}{:02x}", c.r, c.g, c.b));
            let face = facegen_hash([
                &n.facegen_geometry,
                &n.facegen_asymmetry,
                &n.facegen_texture,
            ]);
            rows.push(
                T::Npcs,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    script(),
                    f.opt(n.race),
                    f.opt(n.class),
                    base.map_or(Value::Null, |b| flag(b.flags & 1 != 0)),
                    opt_int(base.map(|b| b.level)),
                    opt_int(base.map(|b| b.flags)),
                    f.opt(n.template),
                    opt_int(base.map(|b| b.template_flags)),
                    f.opt(n.voice),
                    f.opt(n.hair),
                    opt_real(n.hair_length),
                    opt_text(hair_color.as_deref()),
                    f.opt(n.eyes),
                    opt_text(face.as_deref()),
                    opt_real(n.height),
                    opt_real(n.weight),
                    opt_int(n.stats.map(|s| s.base_health)),
                    len(n.head_parts.len()),
                ],
            );
            rows.inventory(&n.inventory);
            rows.packages(&n.packages);
            actor_tables(index, entry, record, &mut rows);
        }
        Record::Creature(c) => {
            // The body model names are relative to the skeleton's folder.
            let skeleton = c.common.model.as_deref().unwrap_or("");
            let first_model = c
                .model_list
                .first()
                .map(|m| creature_model_path(skeleton, m))
                .filter(|p| !p.is_empty());
            let model = match first_model {
                Some(path) => text(path),
                None => model(),
            };
            rows.push(
                T::Creatures,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model,
                    script(),
                    opt_int(c.data.map(|d| d.creature_type)),
                    opt_int(c.base.map(|b| b.level)),
                    opt_int(c.data.map(|d| d.health)),
                    opt_int(c.data.map(|d| d.damage)),
                    opt_real(c.base_scale),
                    len(c.model_list.len()),
                ],
            );
            rows.inventory(&c.inventory);
            rows.packages(&c.packages);
            actor_tables(index, entry, record, &mut rows);
        }
        Record::LandTexture(l) => {
            rows.push(
                T::LandTextures,
                vec![key(), editor_id(), f.opt(l.texture_set)],
            );
        }
        Record::TextureSet(t) => {
            let slot = |i: usize| {
                let path = t.textures[i]
                    .as_deref()
                    .map(normalize_texture_path)
                    .filter(|p| !p.is_empty());
                opt_text(path.as_deref())
            };
            rows.push(
                T::TextureSets,
                vec![
                    key(),
                    editor_id(),
                    slot(0),
                    slot(1),
                    slot(2),
                    slot(3),
                    slot(4),
                    slot(5),
                ],
            );
        }
        Record::Script(s) => {
            let header = s.body.header;
            rows.push(
                T::Scripts,
                vec![
                    key(),
                    editor_id(),
                    opt_int(header.map(|h| h.script_type)),
                    opt_int(header.map(|h| h.variable_count)),
                    opt_int(header.map(|h| h.compiled_size)),
                    opt_text(s.body.source.as_deref()),
                ],
            );
            rows.script_refs("script", &s.body);
            for local in &s.body.locals {
                let row = vec![
                    key(),
                    int(local.index),
                    opt_text(local.name.as_deref()),
                    int(local.flags),
                ];
                rows.push(T::ScriptVariables, row);
            }
        }
        Record::Quest(q) => {
            rows.push(
                T::Quests,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    script(),
                    opt_int(q.data.map(|d| d.flags)),
                    opt_int(q.data.map(|d| d.priority)),
                    len(q.stages.len()),
                    len(q.objectives.len()),
                ],
            );
            for c in &q.conditions {
                rows.condition("quest", c);
            }
            for stage in &q.stages {
                for (number, log) in stage.entries.iter().enumerate() {
                    let context = format!("stage {} entry {number}", stage.index);
                    let row = vec![
                        key(),
                        int(stage.index),
                        len(number),
                        opt_int(log.flags),
                        opt_text(log.text.as_deref()),
                        f.opt(log.next_quest),
                    ];
                    rows.push(T::QuestStages, row);
                    for c in &log.conditions {
                        rows.condition(&context, c);
                    }
                    rows.embedded(&context, &log.script);
                }
            }
            for objective in &q.objectives {
                let row = vec![
                    key(),
                    int(objective.index),
                    opt_text(objective.description.as_deref()),
                    len(objective.targets.len()),
                ];
                rows.push(T::QuestObjectives, row);
                for (number, target) in objective.targets.iter().enumerate() {
                    let context = format!("objective {} target {number}", objective.index);
                    let row = vec![
                        key(),
                        int(objective.index),
                        len(number),
                        f.get(target.target),
                        int(target.flags),
                    ];
                    rows.push(T::QuestTargets, row);
                    for c in &target.conditions {
                        rows.condition(&context, c);
                    }
                }
            }
        }
        Record::Topic(t) => {
            rows.push(
                T::Topics,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    opt_int(t.data.map(|d| d.topic_type)),
                    opt_int(t.data.and_then(|d| d.flags)),
                    opt_real(t.priority),
                    // Filled from the infos by a query; the decoder does not
                    // see them.
                    int(0),
                ],
            );
            for quest in &t.quests {
                let row = vec![key(), f.get(*quest)];
                rows.push(T::TopicQuests, row);
            }
        }
        Record::Info(i) => {
            let topic = match entry.topic {
                Some(k) => text(index.form_key(k).to_string()),
                None => f.opt(i.topic),
            };
            let d = i.data;
            rows.push(
                T::Infos,
                vec![
                    key(),
                    topic,
                    f.opt(i.quest),
                    opt_int(d.map(|d| d.info_type)),
                    opt_int(d.map(|d| d.next_speaker)),
                    opt_int(d.and_then(|d| d.flags)),
                    f.opt(i.speaker),
                    f.opt(i.previous_info),
                    opt_text(i.prompt.as_deref()),
                    len(i.responses.len()),
                    len(i.choices.len()),
                    d.map_or(Value::Null, |d| match (d.flags, d.short) {
                        (None, _) => int(0),
                        (Some(_), true) => int(1),
                        (Some(_), false) => int(2),
                    }),
                    f.opt(i.knam),
                    f.sig(i.knam),
                    opt_int(i.speech_challenge),
                ],
            );
            if d.is_some_and(|d| d.short) {
                rows.skip(
                    "INFO/DATA",
                    "3-byte form has no second flag byte (say-once-a-day and always-darken read as clear)",
                );
            }
            for (kind, list) in [
                ("choice", &i.choices),
                ("link_from", &i.link_from),
                ("follow_up", &i.follow_up),
                ("add_topic", &i.add_topics),
            ] {
                for (seq, topic) in list.iter().enumerate() {
                    let row = vec![key(), text(kind), len(seq), f.get(*topic)];
                    rows.push(T::InfoLinks, row);
                }
            }
            for (seq, response) in i.responses.iter().enumerate() {
                let r = response.data;
                let row = vec![
                    key(),
                    len(seq),
                    opt_int(r.map(|r| r.response_number)),
                    opt_int(r.map(|r| r.emotion_type)),
                    opt_int(r.map(|r| r.emotion_value)),
                    opt_text(response.text.as_deref()),
                ];
                rows.push(T::InfoResponses, row);
            }
            for c in &i.conditions {
                rows.condition("info", c);
            }
            rows.embedded("result begin", &i.result_begin);
            rows.embedded("result end", &i.result_end);
        }
        Record::Terminal(t) => {
            let d = t.data;
            rows.push(
                T::Terminals,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    script(),
                    opt_text(t.header_text.as_deref()),
                    opt_int(d.map(|d| d.difficulty)),
                    opt_int(d.map(|d| d.flags)),
                    opt_int(d.map(|d| d.server_type)),
                    f.opt(t.password_note),
                    f.opt(t.sound),
                    len(t.items.len()),
                ],
            );
            for (seq, item) in t.items.iter().enumerate() {
                let context = format!("item {seq}");
                let source = item
                    .script
                    .source
                    .as_deref()
                    .filter(|s| !s.trim().is_empty());
                let row = vec![
                    key(),
                    len(seq),
                    opt_text(item.text.as_deref()),
                    opt_text(item.result_text.as_deref()),
                    opt_int(item.flags),
                    f.opt(item.display_note),
                    f.opt(item.sub_terminal),
                    opt_text(source),
                    len(item.conditions.len()),
                ];
                rows.push(T::TerminalItems, row);
                for c in &item.conditions {
                    rows.condition(&context, c);
                }
                rows.embedded(&context, &item.script);
            }
        }
        Record::Note(n) => {
            rows.push(
                T::Notes,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    model(),
                    opt_int(n.note_type),
                    opt_text(n.text.as_deref()),
                    f.opt(n.topic),
                    f.opt(n.sound),
                    opt_text(
                        n.texture
                            .as_deref()
                            .map(normalize_texture_path)
                            .filter(|p| !p.is_empty())
                            .as_deref(),
                    ),
                    len(n.quests.len()),
                ],
            );
            for (seq, quest) in n.quests.iter().enumerate() {
                rows.push(T::NoteQuests, vec![key(), len(seq), f.get(*quest)]);
            }
        }
        Record::Package(p) => {
            let d = p.data.as_ref();
            let s = p.schedule;
            let mut row = vec![
                key(),
                editor_id(),
                opt_int(d.map(|d| d.package_type)),
                opt_text(d.and_then(|d| d.type_name())),
                opt_int(d.map(|d| d.flags)),
                opt_int(s.map(|s| s.month)),
                opt_int(s.map(|s| s.day_of_week)),
                opt_int(s.map(|s| s.date)),
                opt_int(s.map(|s| s.hour)),
                opt_int(s.map(|s| s.duration)),
            ];
            row.extend(location_values(f, p.location));
            row.extend(location_values(f, p.location2));
            row.extend(target_values(f, p.target));
            row.extend(target_values(f, p.target2));
            row.push(f.opt(p.dialogue.as_ref().map(|d| d.topic)));
            row.push(f.opt(p.combat_style));
            row.push(len(p.conditions.len()));
            row.push(opt_int(d.map(|d| d.rest.len() as u32 + 5)));
            row.push(opt_int(d.and_then(|d| d.behaviour_flags())));
            row.push(opt_int(d.and_then(|d| d.behaviour_flags_raw())));
            row.push(opt_int(d.and_then(|d| d.type_flags())));
            rows.push(T::Packages, row);
            if let Some(d) = d {
                if d.is_short_form() {
                    rows.skip("PACK/PKDT", "8-byte form has no type-specific flags");
                }
                if d.behaviour_flags_raw().is_some() && d.behaviour_flags().is_none() {
                    rows.skip(
                        "PACK/PKDT",
                        "behaviour flags are the 0xCDCD uninitialised fill (read as not set)",
                    );
                }
            }
            for c in &p.conditions {
                rows.condition("package", c);
            }
            for (name, event) in p.events() {
                rows.embedded(name, &event.script);
                let row = vec![
                    key(),
                    text(name),
                    f.opt(event.idle),
                    f.opt(event.topic),
                    opt_text(
                        event
                            .script
                            .source
                            .as_deref()
                            .filter(|s| !s.trim().is_empty()),
                    ),
                ];
                rows.push(T::PackageEvents, row);
            }
        }
        Record::Global(g) => {
            rows.push(
                T::Globals,
                vec![
                    key(),
                    editor_id(),
                    opt_text(
                        g.value_type
                            .map(|t| char::from(t.code()).to_string())
                            .as_deref(),
                    ),
                    opt_text(g.value_type.map(|t| t.name())),
                    opt_real(g.value),
                ],
            );
        }
        Record::GameSetting(g) => {
            use fnv_esm::GameSettingValue as V;
            let value = g.value.as_ref();
            rows.push(
                T::GameSettings,
                vec![
                    key(),
                    editor_id(),
                    opt_text(value.map(V::kind)),
                    opt_int(value.and_then(V::as_i64)),
                    match value {
                        Some(V::Float(v)) => real(*v),
                        _ => Value::Null,
                    },
                    match value {
                        Some(V::String(text)) => Value::Text(text.clone()),
                        _ => Value::Null,
                    },
                ],
            );
        }
        Record::Message(m) => {
            rows.push(
                T::Messages,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    opt_text(m.text.as_deref()),
                    f.opt(m.icon.filter(|i| !i.is_null())),
                    opt_int(m.flags),
                    opt_int(m.display_time),
                    len(m.buttons.len()),
                ],
            );
            for (seq, button) in m.buttons.iter().enumerate() {
                let row = vec![
                    key(),
                    len(seq),
                    opt_text(button.text.as_deref()),
                    len(button.conditions.len()),
                ];
                rows.push(T::MessageButtons, row);
                for c in &button.conditions {
                    rows.condition(&format!("button {seq}"), c);
                }
            }
        }
        Record::Faction(fa) => {
            rows.push(
                T::Factions,
                vec![
                    key(),
                    editor_id(),
                    name(),
                    opt_int(fa.data.map(|d| d.flags)),
                    opt_int(fa.data.and_then(|d| d.flags2)),
                    len(fa.relations.len()),
                ],
            );
            if fa.data.is_some_and(|d| d.flags2.is_none()) {
                rows.skip("FACT/DATA", "1-byte form has no second flag byte");
            }
            for (seq, relation) in fa.relations.iter().enumerate() {
                let row = vec![
                    key(),
                    len(seq),
                    f.get(relation.faction),
                    int(relation.modifier),
                    opt_int(relation.combat_reaction),
                ];
                rows.push(T::FactionRelations, row);
                if relation.combat_reaction.is_none() {
                    rows.skip("FACT/XNAM", "8-byte relation has no group combat reaction");
                }
            }
        }
        Record::Race(_)
        | Record::HeadPart(_)
        | Record::Hair(_)
        | Record::Eyes(_)
        | Record::ArmorAddon(_)
        | Record::FormList(_)
        | Record::Leveled(_)
        | Record::Unknown(_) => {}
    }
    for (context, body) in record.script_bodies() {
        rows.script_context(&context, body);
    }
    (rows.out, rows.skips)
}

/// Everything one record contributes: its `records` row, its typed rows
/// and its raw subrecords.
fn record_rows(index: &Index<'_>, id: usize, entry: &Entry<'_>, raw: RawStore) -> RecordRows {
    let record = entry.record;
    let form = index.form_key(entry.key).to_string();
    let parent = entry.cell.or(entry.topic).or(entry.world);
    let mut row = vec![
        len(id),
        text(form),
        text(entry.sig().to_string()),
        text(index.plugin_name(entry.plugin)),
        len(record.offset),
        int(record.flags().0),
        flag(record.flags().is_compressed()),
    ];
    let mut out = RecordRows {
        rows: Vec::new(),
        raw: Vec::new(),
        bad: None,
        skips: Vec::new(),
    };
    let keep_raw = match raw {
        RawStore::All => true,
        RawStore::TypedOnly => decoder_for(entry.sig()).is_some(),
        RawStore::None => false,
    };
    let fail = |row: &mut Vec<Value>, error: String| {
        row.extend([Value::Null, Value::Null, Value::Null, Value::Null]);
        row.push(parent.map_or(Value::Null, |k| text(index.form_key(k).to_string())));
        row.extend(std::iter::repeat_n(Value::Null, 7));
        row.push(flag(false));
        Some(index.bad(entry, error))
    };
    let data = match record.data() {
        Ok(data) => data,
        Err(error) => {
            out.bad = fail(&mut row, error.to_string());
            out.rows.push((T::Records, row));
            return out;
        }
    };
    let subrecords = match data.collect() {
        Ok(subs) => subs,
        Err(error) => {
            out.bad = fail(&mut row, error.to_string());
            out.rows.push((T::Records, row));
            return out;
        }
    };
    let common = scan_common(&subrecords);
    let forms = Forms {
        index,
        plugin: entry.plugin,
    };
    let model = common
        .model
        .as_deref()
        .map(normalize_mesh_path)
        .filter(|m| !m.is_empty());
    row.push(opt_text(common.editor_id.as_deref()));
    row.push(opt_text(common.name.as_deref()));
    row.push(opt_text(model.as_deref()));
    row.push(forms.opt(common.script));
    row.push(parent.map_or(Value::Null, |k| text(index.form_key(k).to_string())));
    match common.bounds {
        Some(b) => row.extend(b.iter().map(|&v| int(v))),
        None => row.extend(std::iter::repeat_n(Value::Null, 6)),
    }
    row.push(len(subrecords.len()));
    row.push(flag(true));
    out.rows.push((T::Records, row));
    if keep_raw {
        out.raw = subrecords
            .iter()
            .map(|s| (s.sig, s.data.to_vec()))
            .collect();
    }
    let decoded = fnv_esm::decode_record(entry.sig(), data_meta(entry), &subrecords);
    let (rows, skips) = typed_rows(index, entry, &decoded);
    out.rows.extend(rows);
    out.skips = skips;
    unknown_skips(&decoded, &mut out.skips);
    out
}

/// The script subrecords every embedded script reads.
const SCRIPT_SUBRECORDS: &[&[u8; 4]] = &[
    b"SCHR", b"SCDA", b"SCTX", b"SLSD", b"SCVR", b"SCRO", b"SCRV",
];

/// What the decoders of the simulation records read, by record: a
/// subrecord of one of these left in `unknown` is malformed (a size no
/// layout fits, a repeat, out of place); any other left in `unknown` was
/// not decoded. `None` for a record type this accounting does not cover.
fn read_subrecords(sig: &str) -> Option<&'static [&'static [u8; 4]]> {
    Some(match sig {
        "GLOB" => &[b"FNAM", b"FLTV"],
        "GMST" => &[b"DATA"],
        "MESG" => &[b"DESC", b"INAM", b"DNAM", b"TNAM", b"ITXT", b"CTDA"],
        "FACT" => &[b"DATA", b"XNAM"],
        "PACK" => &[
            b"PKDT", b"PLDT", b"PLD2", b"PSDT", b"PTDT", b"PTD2", b"CTDA", b"PKDD", b"CNAM",
            b"POBA", b"POEA", b"POCA", b"INAM", b"TNAM", b"SCHR", b"SCDA", b"SCTX", b"SLSD",
            b"SCVR", b"SCRO", b"SCRV",
        ],
        "INFO" => &[
            b"DATA", b"QSTI", b"TPIC", b"PNAM", b"NAME", b"TRDT", b"NAM1", b"NAM2", b"NAM3",
            b"SNAM", b"LNAM", b"CTDA", b"TCLT", b"TCLF", b"TCFU", b"NEXT", b"RNAM", b"ANAM",
            b"DNAM", b"KNAM", b"SCHR", b"SCDA", b"SCTX", b"SLSD", b"SCVR", b"SCRO", b"SCRV",
        ],
        "NPC_" | "CREA" => &[b"AIDT", b"VTCK"],
        "QUST" | "TERM" | "SCPT" => SCRIPT_SUBRECORDS,
        _ => return None,
    })
}

/// Counts the subrecords a record's decoder left in `unknown`, for the
/// record types of [`read_subrecords`]: those it reads but could not use
/// (by size), and, for the simulation records themselves, the rest as not
/// decoded.
fn unknown_skips(record: &Record, skips: &mut Vec<(String, String)>) {
    let sig = record.sig().to_string();
    let Some(read) = read_subrecords(&sig) else {
        return;
    };
    let report_rest = !matches!(sig.as_str(), "NPC_" | "CREA" | "QUST" | "TERM" | "SCPT");
    for sub in record.unknown_subrecords() {
        let area = format!("{sig}/{}", sub.sig);
        if read.contains(&&sub.sig.0) {
            skips.push((
                area,
                format!(
                    "malformed, repeated or out of place, size {}",
                    sub.data.len()
                ),
            ));
        } else if report_rest {
            skips.push((area, "not decoded".to_owned()));
        }
    }
}

fn data_meta(entry: &Entry<'_>) -> fnv_esm::RecordMeta {
    entry.record.header.meta
}

fn insert_sql(name: &str, columns: usize) -> String {
    let marks = vec!["?"; columns].join(", ");
    format!("INSERT INTO {name} VALUES ({marks})")
}

fn decode_wave<'a>(
    index: &Index<'a>,
    entries: &[Entry<'a>],
    first_id: usize,
    jobs: usize,
    raw: RawStore,
) -> Vec<RecordRows> {
    let chunks: Vec<(usize, &[Entry<'a>])> = entries
        .chunks(CHUNK)
        .enumerate()
        .map(|(i, c)| (first_id + i * CHUNK, c))
        .collect();
    let work = |(start, chunk): &(usize, &[Entry<'a>])| -> Vec<RecordRows> {
        chunk
            .iter()
            .enumerate()
            .map(|(i, e)| record_rows(index, start + i, e, raw))
            .collect()
    };
    if jobs <= 1 || chunks.len() <= 1 {
        return chunks.iter().flat_map(work).collect();
    }
    thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| scope.spawn(move || work(chunk)))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// Writes `records.sqlite` at `path` (through a temporary file beside it).
pub(crate) fn build(
    path: &Path,
    index: &Index<'_>,
    plugins: &[PluginFile],
    raw: RawStore,
    jobs: usize,
    game: fnv_esm::GameVariant,
) -> Result<BuildStats, MirError> {
    let tmp = path.with_extension("sqlite.tmp");
    if tmp.exists() {
        fs::remove_file(&tmp).map_err(|e| MirError::io(&tmp, e))?;
    }
    let mut conn = Connection::open(&tmp)?;
    conn.execute_batch(
        "PRAGMA page_size = 4096;
         PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA locking_mode = EXCLUSIVE;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -131072;",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    let mut stats = BuildStats::default();
    {
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE plugins (load_order INTEGER PRIMARY KEY, name TEXT NOT NULL,
                                   size INTEGER NOT NULL, sha256 TEXT NOT NULL);",
        )?;
        for (name, columns) in TABLES {
            tx.execute_batch(&format!("CREATE TABLE {name} ({})", columns.join(", ")))?;
        }
        tx.execute_batch(RAW_TABLE)?;
        {
            let mut meta = tx.prepare("INSERT INTO meta VALUES (?1, ?2)")?;
            meta.execute(("schema_version", SCHEMA_VERSION.to_string()))?;
            meta.execute(("raw_store", raw.as_str()))?;
            meta.execute(("records", index.entries().len().to_string()))?;
            meta.execute(("game", game.id()))?;
            meta.execute(("records_revision", RECORDS_REVISION.to_string()))?;
            let mut plugin_row = tx.prepare("INSERT INTO plugins VALUES (?1, ?2, ?3, ?4)")?;
            for (order, file) in plugins.iter().enumerate() {
                plugin_row.execute((
                    order as i64,
                    &file.name,
                    file.bytes.len() as i64,
                    &file.sha256,
                ))?;
            }

            let mut statements = TABLES
                .iter()
                .map(|(name, columns)| tx.prepare(&insert_sql(name, columns.len())))
                .collect::<Result<Vec<_>, _>>()?;
            let mut raw_insert =
                tx.prepare("INSERT INTO raw_subrecords VALUES (?1, ?2, ?3, ?4)")?;
            let mut bad_records: Vec<BadRecord> = index.walk_errors.clone();
            let mut skips: BTreeMap<(String, String), u64> = BTreeMap::new();

            let jobs = jobs.max(1);
            let wave = CHUNK * jobs;
            let entries = index.entries();
            let waves: Vec<(usize, &[Entry<'_>])> = entries
                .chunks(wave)
                .enumerate()
                .map(|(i, c)| (1 + i * wave, c))
                .collect();
            let mut insert = |batch: Vec<RecordRows>| -> Result<(), MirError> {
                for rows in batch {
                    let id = match rows.rows.first().and_then(|(_, v)| v.first()) {
                        Some(Value::Integer(id)) => *id,
                        _ => 0,
                    };
                    for (table, values) in &rows.rows {
                        statements[*table as usize].execute(params_from_iter(values.iter()))?;
                    }
                    for (seq, (sig, data)) in rows.raw.iter().enumerate() {
                        raw_insert.execute((id, seq as i64, sig.to_string(), &data[..]))?;
                        stats.raw_subrecords += 1;
                    }
                    if let Some(bad) = rows.bad {
                        bad_records.push(bad);
                    }
                    for skip in rows.skips {
                        *skips.entry(skip).or_insert(0u64) += 1;
                    }
                    stats.records += 1;
                }
                Ok(())
            };
            // Decode wave n + 1 while wave n is inserted.
            thread::scope(|scope| -> Result<(), MirError> {
                let mut pending: Option<Vec<RecordRows>> = None;
                for &(first, chunk) in &waves {
                    let next = scope.spawn(move || decode_wave(index, chunk, first, jobs, raw));
                    if let Some(batch) = pending.take() {
                        insert(batch)?;
                    }
                    pending = Some(next.join().unwrap_or_default());
                }
                if let Some(batch) = pending.take() {
                    insert(batch)?;
                }
                Ok(())
            })?;

            bad_records
                .sort_by(|a, b| (&a.plugin, a.offset, &a.sig).cmp(&(&b.plugin, b.offset, &b.sig)));
            for (seq, bad) in bad_records.iter().enumerate() {
                statements[T::BadRecords as usize].execute(params_from_iter([
                    len(seq),
                    text(bad.plugin.clone()),
                    int(i64::try_from(bad.offset).unwrap_or(i64::MAX)),
                    text(bad.sig.clone()),
                    opt_text(bad.form.as_ref().map(|f| f.to_string()).as_deref()),
                    text(bad.error.clone()),
                ]))?;
            }
            stats.bad_records = bad_records.len();
            for ((area, reason), count) in &skips {
                statements[T::DecodeSkips as usize].execute(params_from_iter([
                    text(area.clone()),
                    text(reason.clone()),
                    int(i64::try_from(*count).unwrap_or(i64::MAX)),
                ]))?;
            }
        }
        for sql in INDEXES {
            tx.execute_batch(sql)?;
        }
        // Topic info counts, now that every info is in and indexed.
        tx.execute_batch(
            "UPDATE topics SET infos = (SELECT COUNT(*) FROM infos WHERE infos.topic = topics.form)",
        )?;
        tx.commit()?;
    }
    conn.close().map_err(|(_, e)| MirError::Sqlite(e))?;
    fs::rename(&tmp, path).map_err(|e| MirError::io(path, e))?;
    Ok(stats)
}
