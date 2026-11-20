//! Building `records.sqlite`: the typed core tables and the lossless raw
//! store, for every record of the plugin list.
//!
//! The file is written in one transaction, rows in form order, indexes
//! last, so that two builds from the same plugins are byte-identical.
//! Records are decoded on `jobs` threads, a wave at a time, while the
//! previous wave is inserted; the insert statements are prepared once.

use std::fs;
use std::path::Path;
use std::thread;

use fnv_esm::{Condition, FormId, Record, Sig, decoder_for};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use sha2::{Digest, Sha256};

use crate::error::MirError;
use crate::index::{BadRecord, Entry, Index, PluginFile, scan_common};
use crate::model::{RawStore, SCHEMA_VERSION};
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
        "ammo TEXT",
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
];

/// Records decoded per thread and wave.
const CHUNK: usize = 2048;

/// What one record contributes to the file.
struct RecordRows {
    rows: Vec<(T, Vec<Value>)>,
    raw: Vec<(Sig, Vec<u8>)>,
    bad: Option<BadRecord>,
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
}

impl Rows<'_, '_, '_> {
    fn push(&mut self, table: T, values: Vec<Value>) {
        self.out.push((table, values));
    }

    fn condition(&mut self, context: &str, c: &Condition) {
        let (value, global) = match c.value {
            fnv_esm::ConditionValue::Float(v) => (real(v), Value::Null),
            fnv_esm::ConditionValue::Global(g) => (Value::Null, self.forms.get(g)),
        };
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
    }
}

/// The typed rows of one decoded record.
fn typed_rows(index: &Index<'_>, entry: &Entry<'_>, record: &Record) -> Vec<(T, Vec<Value>)> {
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
                ],
            );
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
        Record::Race(_)
        | Record::HeadPart(_)
        | Record::Hair(_)
        | Record::Eyes(_)
        | Record::ArmorAddon(_)
        | Record::FormList(_)
        | Record::Leveled(_)
        | Record::Unknown(_) => {}
    }
    rows.out
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
    out.rows.extend(typed_rows(index, entry, &decoded));
    out
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
