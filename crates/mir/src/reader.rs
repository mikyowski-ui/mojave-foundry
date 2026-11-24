//! Reading a MIR directory: [`Mir`] for the documents and assets, and
//! [`Records`] for typed queries over `records.sqlite`.
//!
//! Later stages read the directory only through this module.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::assets::decode_png;
use crate::error::MirError;
use crate::model::{
    ActorDoc, ActorEntry, AnimationInfo, CellDoc, CellKey, Image, InteriorEntry, Manifest,
    SCHEMA_VERSION, SkeletonInfo,
};
use crate::paths::FormKey;

/// File names inside a MIR directory.
pub(crate) const MANIFEST: &str = "manifest.json";
pub(crate) const RECORDS: &str = "records.sqlite";
pub(crate) const REPORT: &str = "report.md";
pub(crate) const MESHES: &str = "meshes";
pub(crate) const TEXTURES: &str = "textures";
pub(crate) const CELLS: &str = "cells";
pub(crate) const INTERIORS: &str = "interiors";
pub(crate) const ACTORS: &str = "actors";
pub(crate) const SKELETONS: &str = "skeletons";
pub(crate) const ANIMATIONS: &str = "animations";

/// Relative path of an exterior cell document.
pub(crate) fn cell_path(worldspace: &str, x: i32, y: i32) -> PathBuf {
    Path::new(CELLS)
        .join(worldspace.to_ascii_lowercase())
        .join(format!("{x}_{y}.json"))
}

/// Relative path of an interior cell document.
pub(crate) fn interior_path(id: &str) -> PathBuf {
    Path::new(INTERIORS).join(format!("{id}.json"))
}

/// Relative path of an actor document.
pub(crate) fn actor_path(id: &str) -> PathBuf {
    Path::new(ACTORS).join(format!("{id}.json"))
}

fn is_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(
    dir: &Path,
    relative: &Path,
) -> Result<T, MirError> {
    let path = dir.join(relative);
    let bytes = fs::read(&path).map_err(|e| MirError::io(&path, e))?;
    serde_json::from_slice(&bytes).map_err(|source| MirError::Json {
        path: relative.display().to_string().replace('\\', "/"),
        source,
    })
}

/// An opened MIR directory.
#[derive(Debug, Clone)]
pub struct Mir {
    dir: PathBuf,
    manifest: Manifest,
}

impl Mir {
    /// Opens a directory written by [`crate::extract`].
    ///
    /// # Errors
    ///
    /// [`MirError::Io`] or [`MirError::Json`] when `manifest.json` is
    /// missing or does not parse, and [`MirError::SchemaVersion`] when it
    /// was written by another schema version.
    pub fn open(dir: &Path) -> Result<Self, MirError> {
        let manifest: Manifest = read_json(dir, Path::new(MANIFEST))?;
        if manifest.schema_version != SCHEMA_VERSION {
            return Err(MirError::SchemaVersion {
                path: MANIFEST.to_owned(),
                found: manifest.schema_version,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            manifest,
        })
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The manifest: inputs, cells, and the path → hash index.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The game the directory was extracted from ([`Manifest::game`]); a
    /// directory written before the game was recorded is New Vegas's.
    pub fn game(&self) -> fnv_esm::GameVariant {
        self.manifest.game
    }

    /// The exterior cells of the directory, sorted.
    pub fn cells(&self) -> impl Iterator<Item = CellKey> + '_ {
        self.manifest.cells.iter().cloned()
    }

    /// The interior cells of the directory, sorted by id.
    pub fn interiors(&self) -> impl Iterator<Item = &InteriorEntry> + '_ {
        self.manifest.interiors.iter()
    }

    /// An exterior cell; `None` when the directory does not have it.
    /// The worldspace's case does not matter.
    ///
    /// # Errors
    ///
    /// When the cell's file cannot be read or parsed.
    pub fn cell(&self, worldspace: &str, x: i32, y: i32) -> Result<Option<CellDoc>, MirError> {
        let key = CellKey {
            worldspace: worldspace.to_ascii_lowercase(),
            x,
            y,
        };
        if self.manifest.cells.binary_search(&key).is_err() {
            return Ok(None);
        }
        let doc: CellDoc = read_json(&self.dir, &cell_path(&key.worldspace, x, y))?;
        check_doc(&doc, &cell_path(&key.worldspace, x, y))?;
        Ok(Some(doc))
    }

    /// An interior cell by editor id (case does not matter) or by the id
    /// the manifest lists; `None` when the directory does not have it.
    ///
    /// # Errors
    ///
    /// When the cell's file cannot be read or parsed.
    pub fn interior(&self, editor_id: &str) -> Result<Option<CellDoc>, MirError> {
        let id = crate::cells::interior_id(Some(editor_id), &FormKey::new("", 0));
        let Some(entry) = self.manifest.interiors.iter().find(|e| e.id == id) else {
            return Ok(None);
        };
        let doc: CellDoc = read_json(&self.dir, &interior_path(&entry.id))?;
        check_doc(&doc, &interior_path(&entry.id))?;
        Ok(Some(doc))
    }

    /// The interior cell with the given cell form, if the directory has it.
    ///
    /// # Errors
    ///
    /// When the cell's file cannot be read or parsed.
    pub fn interior_by_form(&self, form: &FormKey) -> Result<Option<CellDoc>, MirError> {
        match self.manifest.interiors.iter().find(|e| &e.form == form) {
            Some(entry) => {
                let path = interior_path(&entry.id);
                let doc: CellDoc = read_json(&self.dir, &path)?;
                check_doc(&doc, &path)?;
                Ok(Some(doc))
            }
            None => Ok(None),
        }
    }

    /// The actor documents of the directory, sorted by form: one per
    /// distinct actor base the slice's references place.
    pub fn actors(&self) -> impl Iterator<Item = &ActorEntry> + '_ {
        self.manifest.actors.iter()
    }

    /// The actor document of a placed base form; `None` when the directory
    /// has none.
    ///
    /// # Errors
    ///
    /// When the document cannot be read or parsed, or was written by
    /// another schema version.
    pub fn actor(&self, form: &FormKey) -> Result<Option<ActorDoc>, MirError> {
        let Ok(at) = self.manifest.actors.binary_search_by(|e| e.form.cmp(form)) else {
            return Ok(None);
        };
        let path = actor_path(&self.manifest.actors[at].id);
        let doc: ActorDoc = read_json(&self.dir, &path)?;
        if doc.schema_version != SCHEMA_VERSION {
            return Err(MirError::SchemaVersion {
                path: path.display().to_string().replace('\\', "/"),
                found: doc.schema_version,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(Some(doc))
    }

    /// A converted mesh by content hash.
    ///
    /// # Errors
    ///
    /// [`MirError::Mesh`] when the hash is malformed or the file does not
    /// read back; [`MirError::Io`] when it cannot be read.
    pub fn mesh(&self, hash: &str) -> Result<nif::StaticMesh, MirError> {
        let bad = |reason: &str| MirError::Mesh {
            hash: hash.to_owned(),
            source: nif::NifError::Glb {
                offset: 0,
                reason: reason.to_owned(),
            },
        };
        if !is_hash(hash) {
            return Err(bad("not a content hash"));
        }
        let path = self.dir.join(MESHES).join(format!("{hash}.glb"));
        let bytes = fs::read(&path).map_err(|e| MirError::io(&path, e))?;
        nif::glb::read(&bytes).map_err(|source| MirError::Mesh {
            hash: hash.to_owned(),
            source,
        })
    }

    /// A converted texture by content hash, as RGBA8.
    ///
    /// # Errors
    ///
    /// [`MirError::Texture`] when the hash is malformed or the file does
    /// not decode; [`MirError::Io`] when it cannot be read.
    pub fn texture(&self, hash: &str) -> Result<Image, MirError> {
        if !is_hash(hash) {
            return Err(MirError::Texture {
                hash: hash.to_owned(),
                reason: "not a content hash".to_owned(),
            });
        }
        let path = self.dir.join(TEXTURES).join(format!("{hash}.png"));
        let bytes = fs::read(&path).map_err(|e| MirError::io(&path, e))?;
        decode_png(&bytes).map_err(|reason| MirError::Texture {
            hash: hash.to_owned(),
            reason,
        })
    }

    /// A converted skeleton by content hash (an actor document's
    /// `skeleton`).
    ///
    /// # Errors
    ///
    /// [`MirError::Document`] when the hash is malformed, [`MirError::Io`]
    /// when the file cannot be read and [`MirError::Json`] when it does not
    /// parse.
    pub fn skeleton(&self, hash: &str) -> Result<nif::Skeleton, MirError> {
        self.document(SKELETONS, hash)
    }

    /// A converted animation by content hash (an actor document's
    /// `animations.stand` or `animations.walk`).
    ///
    /// # Errors
    ///
    /// [`MirError::Document`] when the hash is malformed, [`MirError::Io`]
    /// when the file cannot be read and [`MirError::Json`] when it does not
    /// parse.
    pub fn animation(&self, hash: &str) -> Result<nif::Animation, MirError> {
        self.document(ANIMATIONS, hash)
    }

    fn document<T: serde::de::DeserializeOwned>(
        &self,
        store: &'static str,
        hash: &str,
    ) -> Result<T, MirError> {
        if !is_hash(hash) {
            return Err(MirError::Document {
                store,
                hash: hash.to_owned(),
                reason: "not a content hash".to_owned(),
            });
        }
        read_json(&self.dir, &Path::new(store).join(format!("{hash}.json")))
    }

    /// The content hash of a converted skeleton, by path (normalised first,
    /// as a model path).
    pub fn skeleton_hash(&self, path: &str) -> Option<&str> {
        self.manifest.skeleton_hash(path)
    }

    /// The content hash of a converted animation, by `.kf` path (normalised
    /// first, as a model path).
    pub fn animation_hash(&self, path: &str) -> Option<&str> {
        self.manifest.animation_hash(path)
    }

    /// The manifest's facts about a converted skeleton, by content hash.
    pub fn skeleton_info(&self, hash: &str) -> Option<&SkeletonInfo> {
        self.manifest.skeleton_info.get(hash)
    }

    /// The manifest's facts about a converted animation, by content hash.
    pub fn animation_info(&self, hash: &str) -> Option<&AnimationInfo> {
        self.manifest.animation_info.get(hash)
    }

    /// The content hash of a converted texture, by path. The path is
    /// normalised first, so a mesh material's path works as it is.
    pub fn texture_hash(&self, path: &str) -> Option<&str> {
        self.manifest.texture_hash(path)
    }

    /// The content hash of a converted mesh, by model path (normalised
    /// first).
    pub fn mesh_hash(&self, model: &str) -> Option<&str> {
        self.manifest.mesh_hash(model)
    }

    /// Opens `records.sqlite` for typed queries.
    ///
    /// # Errors
    ///
    /// [`MirError::Sqlite`] when the file does not open, and
    /// [`MirError::SchemaVersion`] when it has another schema version.
    pub fn records(&self) -> Result<Records, MirError> {
        Records::open(&self.dir.join(RECORDS))
    }

    /// The text of `report.md`, as the extract wrote it.
    ///
    /// # Errors
    ///
    /// When the file cannot be read.
    pub fn report_text(&self) -> Result<String, MirError> {
        let path = self.dir.join(REPORT);
        fs::read_to_string(&path).map_err(|e| MirError::io(&path, e))
    }
}

fn check_doc(doc: &CellDoc, path: &Path) -> Result<(), MirError> {
    if doc.schema_version == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(MirError::SchemaVersion {
            path: path.display().to_string().replace('\\', "/"),
            found: doc.schema_version,
            expected: SCHEMA_VERSION,
        })
    }
}

/// One row of the `records` table: what every record has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRow {
    /// The record.
    pub form: FormKey,
    /// Its signature, for example `"STAT"`.
    pub sig: String,
    /// The plugin that won the record, lowercase.
    pub plugin: String,
    /// Byte offset of the record header in that plugin.
    pub offset: u64,
    /// Record flags, as stored.
    pub flags: u32,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Model path, normalised.
    pub model: Option<String>,
    /// The enclosing cell, topic or worldspace.
    pub parent: Option<FormKey>,
    /// Whether the record's data could be read.
    pub decoded: bool,
}

/// What a map builder needs to know about a door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoorInfo {
    /// Door flags, as stored.
    pub flags: Option<u8>,
    /// Open sound.
    pub open_sound: Option<FormKey>,
    /// Close sound.
    pub close_sound: Option<FormKey>,
}

/// What a map builder needs to know about a container.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerInfo {
    /// Container flags, as stored.
    pub flags: Option<u8>,
    /// Weight.
    pub weight: Option<f32>,
    /// Contents: item and count, in file order.
    pub items: Vec<(Option<FormKey>, i32)>,
}

/// A base object: anything a reference can place.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseObject {
    /// The form.
    pub form: FormKey,
    /// Record signature: `"STAT"`, `"DOOR"`, `"CONT"`, `"NPC_"`, ...
    pub kind: String,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Model path, normalised. For an armour, its world model.
    pub model: Option<String>,
    /// Attached script.
    pub script: Option<FormKey>,
    /// Object bounds, `[min, max]`.
    pub bounds: Option<[[i16; 3]; 2]>,
    /// Value in caps, for items.
    pub value: Option<i64>,
    /// Weight, for items and containers.
    pub weight: Option<f32>,
    /// Door details, for a `DOOR`.
    pub door: Option<DoorInfo>,
    /// Container details, for a `CONT`.
    pub container: Option<ContainerInfo>,
}

/// A placed reference, as `placed_refs` stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedRow {
    /// The reference.
    pub form: FormKey,
    /// `"object"`, `"npc"` or `"creature"`.
    pub kind: String,
    /// Base form.
    pub base: Option<FormKey>,
    /// Base form's signature.
    pub base_sig: Option<String>,
    /// Enclosing cell.
    pub cell: Option<FormKey>,
    /// Enclosing worldspace.
    pub worldspace: Option<FormKey>,
    /// Position, when the record has one.
    pub position: Option<[f64; 3]>,
    /// Rotation, when the record has one.
    pub rotation: Option<[f64; 3]>,
    /// Scale.
    pub scale: f64,
    /// Teleport destination door.
    pub teleport_door: Option<FormKey>,
}

/// A script and its source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRow {
    /// The script record.
    pub form: FormKey,
    /// Editor id.
    pub editor_id: Option<String>,
    /// Script type, as stored.
    pub script_type: Option<i64>,
    /// Source text.
    pub source: Option<String>,
    /// Declared local variable names.
    pub variables: Vec<String>,
}

/// An NPC's appearance and outfit, for clustering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpcAppearance {
    /// The NPC.
    pub form: FormKey,
    /// Race, sex, hair, hair colour, eyes and FaceGen hash, joined.
    pub appearance: String,
    /// Sorted armour items of the inventory, joined.
    pub outfit: String,
}

/// A quest stage log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestStageRow {
    /// Stage index.
    pub stage: i64,
    /// Log entry number within the stage.
    pub entry: i64,
    /// Entry flags.
    pub flags: Option<i64>,
    /// Log text.
    pub text: Option<String>,
}

/// A record that could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadRecordRow {
    /// Plugin file name.
    pub plugin: String,
    /// Byte offset in the plugin.
    pub offset: u64,
    /// Signature.
    pub sig: String,
    /// Form, when known.
    pub form: Option<String>,
    /// The error.
    pub error: String,
}

/// Typed queries over `records.sqlite`, opened read-only.
#[derive(Debug)]
pub struct Records {
    conn: Connection,
}

fn form(text: Option<String>) -> Option<FormKey> {
    text.and_then(|t| t.parse().ok())
}

impl Records {
    /// Opens a `records.sqlite` file read-only.
    ///
    /// # Errors
    ///
    /// When it does not open or has another schema version.
    pub fn open(path: &Path) -> Result<Self, MirError> {
        if !path.is_file() {
            return Err(MirError::io(
                path,
                std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
            ));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            return Err(MirError::SchemaVersion {
                path: RECORDS.to_owned(),
                found: version,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(Self { conn })
    }

    /// The connection, for queries this type has no method for. The
    /// schema is described in the crate documentation.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The `records` row of a form.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn record(&self, key: &FormKey) -> Result<Option<RecordRow>, MirError> {
        Ok(self
            .conn
            .query_row(
                "SELECT form, sig, plugin, offset, flags, editor_id, name, model, parent, decoded \
                 FROM records WHERE form = ?1",
                [key.to_string()],
                |r| {
                    Ok(RecordRow {
                        form: form(r.get(0)?).unwrap_or_else(|| key.clone()),
                        sig: r.get(1)?,
                        plugin: r.get(2)?,
                        offset: r.get::<_, i64>(3)? as u64,
                        flags: r.get::<_, i64>(4)? as u32,
                        editor_id: r.get(5)?,
                        name: r.get(6)?,
                        model: r.get(7)?,
                        parent: form(r.get(8)?),
                        decoded: r.get(9)?,
                    })
                },
            )
            .optional()?)
    }

    /// The forms whose editor id is `editor_id` (case does not matter),
    /// sorted.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn find_editor_id(&self, editor_id: &str) -> Result<Vec<FormKey>, MirError> {
        let mut statement = self.conn.prepare(
            "SELECT form FROM records WHERE editor_id = ?1 COLLATE NOCASE ORDER BY form",
        )?;
        let rows = statement.query_map([editor_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.extend(form(Some(row?)));
        }
        out.sort();
        Ok(out)
    }

    /// A base object with what a map builder needs: kind, names, model,
    /// value and weight for items, and door or container details.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn base_object(&self, key: &FormKey) -> Result<Option<BaseObject>, MirError> {
        let row = self
            .conn
            .query_row(
                "SELECT sig, editor_id, name, model, script, bound_min_x, bound_min_y, \
                 bound_min_z, bound_max_x, bound_max_y, bound_max_z FROM records WHERE form = ?1",
                [key.to_string()],
                |r| {
                    let b: [Option<i64>; 6] = [
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                    ];
                    let bounds = match b {
                        [Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)] => {
                            let v = |x: i64| x as i16;
                            Some([[v(a), v(b), v(c)], [v(d), v(e), v(f)]])
                        }
                        _ => None,
                    };
                    Ok(BaseObject {
                        form: key.clone(),
                        kind: r.get(0)?,
                        editor_id: r.get(1)?,
                        name: r.get(2)?,
                        model: r.get(3)?,
                        script: form(r.get(4)?),
                        bounds,
                        value: None,
                        weight: None,
                        door: None,
                        container: None,
                    })
                },
            )
            .optional()?;
        let Some(mut object) = row else {
            return Ok(None);
        };
        let k = key.to_string();
        let item_table = match object.kind.as_str() {
            "MISC" => Some("misc_items"),
            "WEAP" => Some("weapons"),
            "ARMO" => Some("armors"),
            "ALCH" => Some("ingestibles"),
            "AMMO" => Some("ammo"),
            _ => None,
        };
        if let Some(table) = item_table {
            let sql = format!("SELECT value, weight, model FROM {table} WHERE form = ?1");
            if let Some((value, weight, model)) = self
                .conn
                .query_row(&sql, [&k], |r| {
                    Ok((
                        r.get::<_, Option<i64>>(0)?,
                        r.get::<_, Option<f64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .optional()?
            {
                object.value = value;
                object.weight = weight.map(|w| w as f32);
                if model.is_some() {
                    object.model = model;
                }
            }
        }
        match object.kind.as_str() {
            "DOOR" => {
                object.door = self
                    .conn
                    .query_row(
                        "SELECT flags, open_sound, close_sound FROM doors WHERE form = ?1",
                        [&k],
                        |r| {
                            Ok(DoorInfo {
                                flags: r.get::<_, Option<i64>>(0)?.map(|f| f as u8),
                                open_sound: form(r.get(1)?),
                                close_sound: form(r.get(2)?),
                            })
                        },
                    )
                    .optional()?;
            }
            "CONT" => {
                let head = self
                    .conn
                    .query_row(
                        "SELECT flags, weight FROM containers WHERE form = ?1",
                        [&k],
                        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<f64>>(1)?)),
                    )
                    .optional()?;
                if let Some((flags, weight)) = head {
                    object.weight = weight.map(|w| w as f32);
                    object.container = Some(ContainerInfo {
                        flags: flags.map(|f| f as u8),
                        weight: weight.map(|w| w as f32),
                        items: self.inventory(key)?,
                    });
                }
            }
            _ => {}
        }
        Ok(Some(object))
    }

    /// The inventory of a container, NPC or creature: item and count, in
    /// file order.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn inventory(&self, owner: &FormKey) -> Result<Vec<(Option<FormKey>, i32)>, MirError> {
        let mut statement = self
            .conn
            .prepare("SELECT item, count FROM inventory WHERE owner = ?1 ORDER BY seq")?;
        let rows = statement.query_map([owner.to_string()], |r| {
            Ok((form(r.get(0)?), r.get::<_, i64>(1)? as i32))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// A placed reference.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn placed_ref(&self, key: &FormKey) -> Result<Option<PlacedRow>, MirError> {
        Ok(self
            .conn
            .query_row(
                "SELECT kind, base, base_sig, cell, worldspace, x, y, z, rx, ry, rz, scale, \
                 teleport_door FROM placed_refs WHERE form = ?1",
                [key.to_string()],
                |r| {
                    let triple = |a: Option<f64>, b: Option<f64>, c: Option<f64>| match (a, b, c) {
                        (Some(a), Some(b), Some(c)) => Some([a, b, c]),
                        _ => None,
                    };
                    Ok(PlacedRow {
                        form: key.clone(),
                        kind: r.get(0)?,
                        base: form(r.get(1)?),
                        base_sig: r.get(2)?,
                        cell: form(r.get(3)?),
                        worldspace: form(r.get(4)?),
                        position: triple(r.get(5)?, r.get(6)?, r.get(7)?),
                        rotation: triple(r.get(8)?, r.get(9)?, r.get(10)?),
                        scale: r.get::<_, Option<f64>>(11)?.unwrap_or(1.0),
                        teleport_door: form(r.get(12)?),
                    })
                },
            )
            .optional()?)
    }

    fn counts(&self, sql: &str) -> Result<BTreeMap<String, u64>, MirError> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?
                    .unwrap_or_else(|| "(none)".to_owned()),
                r.get::<_, i64>(1)? as u64,
            ))
        })?;
        let mut out = BTreeMap::new();
        for row in rows {
            let (key, count) = row?;
            *out.entry(key).or_insert(0) += count;
        }
        Ok(out)
    }

    /// Number of records by signature.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn record_counts(&self) -> Result<BTreeMap<String, u64>, MirError> {
        self.counts("SELECT sig, COUNT(*) FROM records GROUP BY sig")
    }

    /// Number of placed references by the signature of their base;
    /// `"(none)"` for a base outside the plugin list.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn reference_counts(&self) -> Result<BTreeMap<String, u64>, MirError> {
        self.counts("SELECT base_sig, COUNT(*) FROM placed_refs GROUP BY base_sig")
    }

    /// Number of conditions by function index, over every condition.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn condition_function_counts(&self) -> Result<BTreeMap<u32, u64>, MirError> {
        let mut statement = self
            .conn
            .prepare("SELECT function, COUNT(*) FROM conditions GROUP BY function")?;
        let rows = statement.query_map([], |r| {
            Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u64))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every script record, with its source and declared variables, in
    /// form order.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn scripts(&self) -> Result<Vec<ScriptRow>, MirError> {
        let mut variables: BTreeMap<String, Vec<String>> = BTreeMap::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT script, name FROM script_variables WHERE name IS NOT NULL ORDER BY script, idx",
            )?;
            let rows = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (script, name) = row?;
                variables.entry(script).or_default().push(name);
            }
        }
        let mut statement = self
            .conn
            .prepare("SELECT form, editor_id, script_type, source FROM scripts ORDER BY form")?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (key, editor_id, script_type, source) = row?;
            let Some(parsed) = form(Some(key.clone())) else {
                continue;
            };
            out.push(ScriptRow {
                form: parsed,
                editor_id,
                script_type,
                source,
                variables: variables.remove(&key).unwrap_or_default(),
            });
        }
        out.sort_by(|a, b| a.form.cmp(&b.form));
        Ok(out)
    }

    /// The source of one script record.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn script_source(&self, key: &FormKey) -> Result<Option<String>, MirError> {
        Ok(self
            .conn
            .query_row(
                "SELECT source FROM scripts WHERE form = ?1",
                [key.to_string()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Result scripts embedded in quests and dialogue: owner, context and
    /// source, in owner order.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn embedded_scripts(&self) -> Result<Vec<(FormKey, String, String)>, MirError> {
        let mut statement = self
            .conn
            .prepare("SELECT owner, context, source FROM embedded_scripts ORDER BY owner, rowid")?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (owner, context, source) = row?;
            if let Some(owner) = form(Some(owner)) {
                out.push((owner, context, source));
            }
        }
        Ok(out)
    }

    /// Every editor id of the plugin list, lowercase, sorted, distinct.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn editor_ids(&self) -> Result<Vec<String>, MirError> {
        let mut statement = self
            .conn
            .prepare("SELECT DISTINCT lower(editor_id) FROM records WHERE editor_id IS NOT NULL")?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        let mut out: Vec<String> = rows.collect::<Result<_, _>>()?;
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// The stages of a quest, in stage and entry order.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn quest_stages(&self, quest: &FormKey) -> Result<Vec<QuestStageRow>, MirError> {
        let mut statement = self.conn.prepare(
            "SELECT stage, entry, flags, text FROM quest_stages WHERE quest = ?1 ORDER BY stage, entry",
        )?;
        let rows = statement.query_map([quest.to_string()], |r| {
            Ok(QuestStageRow {
                stage: r.get(0)?,
                entry: r.get(1)?,
                flags: r.get(2)?,
                text: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The infos of a dialogue topic, in form order.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn topic_infos(&self, topic: &FormKey) -> Result<Vec<FormKey>, MirError> {
        let mut statement = self
            .conn
            .prepare("SELECT form FROM infos WHERE topic = ?1 ORDER BY form")?;
        let rows = statement.query_map([topic.to_string()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.extend(form(Some(row?)));
        }
        out.sort();
        Ok(out)
    }

    /// The raw subrecords of a record, in file order: signature and bytes.
    /// Empty when the raw store does not hold the record.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn raw_subrecords(&self, key: &FormKey) -> Result<Vec<(String, Vec<u8>)>, MirError> {
        let mut statement = self.conn.prepare(
            "SELECT r.sig, r.data FROM raw_subrecords r JOIN records ON records.id = r.record \
             WHERE records.form = ?1 ORDER BY r.seq",
        )?;
        let rows = statement.query_map(params![key.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Appearance and outfit signatures of every NPC, in form order. The
    /// outfit is the sorted list of armour items in the inventory.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn npc_appearances(&self) -> Result<Vec<NpcAppearance>, MirError> {
        let mut outfits: BTreeMap<String, Vec<String>> = BTreeMap::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT inventory.owner, inventory.item FROM inventory \
                 JOIN records ON records.form = inventory.item \
                 JOIN npcs ON npcs.form = inventory.owner \
                 WHERE records.sig = 'ARMO'",
            )?;
            let rows = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (owner, item) = row?;
                outfits.entry(owner).or_default().push(item);
            }
        }
        let mut statement = self.conn.prepare(
            "SELECT form, race, female, hair, hair_color, eyes, facegen FROM npcs ORDER BY form",
        )?;
        let rows = statement.query_map([], |r| {
            let text = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
            let female: Option<bool> = r.get(2)?;
            Ok((
                r.get::<_, String>(0)?,
                [
                    text(r.get(1)?),
                    match female {
                        Some(true) => "f".to_owned(),
                        Some(false) => "m".to_owned(),
                        None => "-".to_owned(),
                    },
                    text(r.get(3)?),
                    text(r.get(4)?),
                    text(r.get(5)?),
                    text(r.get(6)?),
                ]
                .join("|"),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (key, appearance) = row?;
            let mut outfit = outfits.remove(&key).unwrap_or_default();
            outfit.sort();
            outfit.dedup();
            if let Some(form) = form(Some(key)) {
                out.push(NpcAppearance {
                    form,
                    appearance,
                    outfit: outfit.join(","),
                });
            }
        }
        out.sort_by(|a, b| a.form.cmp(&b.form));
        Ok(out)
    }

    /// The records that could not be decoded, in plugin and offset order.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn bad_records(&self) -> Result<Vec<BadRecordRow>, MirError> {
        let mut statement = self
            .conn
            .prepare("SELECT plugin, offset, sig, form, error FROM bad_records ORDER BY seq")?;
        let rows = statement.query_map([], |r| {
            Ok(BadRecordRow {
                plugin: r.get(0)?,
                offset: r.get::<_, i64>(1)? as u64,
                sig: r.get(2)?,
                form: r.get(3)?,
                error: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// A value of the `meta` table (`schema_version`, `raw_store`,
    /// `records`).
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn meta(&self, key: &str) -> Result<Option<String>, MirError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }
}
