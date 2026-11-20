//! The MIR directory: what mojave-foundry extracts from an install, and
//! the only way later stages read it.
//!
//! [`extract`] reads the plugins of an install and writes a directory
//! (`docs/contracts.md`, section 2):
//!
//! ```text
//! <mir>/
//!   records.sqlite                     typed core tables + lossless raw subrecords
//!   meshes/<hash>.glb                  one per distinct source mesh
//!   textures/<hash>.png                one per distinct source texture, RGBA8
//!   cells/<worldspace>/<x>_<y>.json    exterior cells, editor id lowercase
//!   interiors/<editor_id>.json         interior cells, editor id lowercase
//!   actors/<plugin>_<local id>.json    placed actor bases, resolved (`ActorDoc`)
//!   skeletons/<hash>.json              one per distinct actor skeleton (`nif::Skeleton`)
//!   animations/<hash>.json             one per distinct stand or walk `.kf` (`nif::Animation`)
//!   report.md
//!   manifest.json                      path -> hash index and the extract inputs
//! ```
//!
//! [`Mir`] opens such a directory: cell documents ([`CellDoc`]), actor
//! documents ([`ActorDoc`]), meshes (`nif::StaticMesh`), textures
//! ([`Image`]), skeletons (`nif::Skeleton`), animations (`nif::Animation`)
//! and typed record queries ([`Records`]). [`report`] writes `report.md`. [`fixture::mini_town`]
//! writes a small synthetic install to run the whole pipeline on.
//!
//! # `records.sqlite`
//!
//! Every record of the plugin list, the winning version of each form,
//! inserted in form order. Form columns hold form keys
//! (`"falloutnv.esm:0daebb"`), model and texture paths are normalised.
//!
//! - `meta(key, value)`: `schema_version`, `raw_store`, `records`;
//!   `plugins(load_order, name, size, sha256)`.
//! - `records(id, form, sig, plugin, offset, flags, compressed, editor_id,
//!   name, model, script, parent, bound_*, subrecords, decoded)`: one row
//!   per record of every kind. `parent` is the enclosing cell, topic or
//!   worldspace.
//! - `raw_subrecords(record, seq, sig, data)`: the lossless store,
//!   `record` being `records.id`; what it holds follows [`RawStore`].
//! - World: `worldspaces`, `cells` (worldspace, grid, interior,
//!   persistent), `placed_refs` (base, base kind, cell, position,
//!   rotation, scale, flags, teleport, enable parent, lock, owner),
//!   `lands`, `navmeshes`.
//! - Objects: `statics`, `doors`, `containers`, `misc_items`, `weapons`,
//!   `armors`, `ingestibles`, `ammo`, `npcs` (appearance fields and a
//!   FaceGen hash), `creatures`, `inventory(owner, seq, item, count)`,
//!   `land_textures`, `texture_sets`.
//! - Logic: `scripts` (with source text), `script_variables`,
//!   `embedded_scripts` (quest and dialogue result scripts), `quests`,
//!   `quest_stages`, `quest_objectives`, `topics`, `topic_quests`,
//!   `infos`, `info_responses`, `conditions(owner, seq, context,
//!   function, comparison, value, global, param1, param2, run_on,
//!   reference, is_or)`.
//! - `bad_records(seq, plugin, offset, sig, form, error)`.
//!
//! The file is byte-identical across two builds from the same plugins.

mod actors;
mod assets;
mod cells;
mod db;
mod error;
mod extract;
mod finite;
pub mod fixture;
pub mod ground;
mod index;
mod model;
mod paths;
mod reader;
mod report;
mod source;

pub use actors::{
    HUMAN_SKELETON, HUMAN_STAND, HUMAN_WALK_FEMALE, HUMAN_WALK_MALE, MAX_CHAIN, USE_BASE_DATA,
    USE_INVENTORY, USE_MODEL, USE_STATS, USE_TRAITS,
};
pub use assets::AssetCounts;
pub use error::MirError;
pub use extract::{ExtractOptions, ExtractSummary, extract};
pub use ground::{GroundSampler, MAP_SIDE_SQUARES, MapWindow};
pub use index::content_hash;
pub use model::{
    ActorAnimations, ActorDoc, ActorEntry, ActorKind, ActorPart, ActorTint, AnimationInfo,
    ArchiveEntry, AssetEntry, AssetStatus, CELL_UNITS, CellDoc, CellKey, CellLocation, CellRef,
    EnableParentDoc, Image, InteriorEntry, Issue, LAND_GRID, LAND_SPACING, LandDoc, LandLayer,
    LayerOpacity, LockDoc, Manifest, MeshInfo, NavDoorLinkDoc, NavExternalLinkDoc, NavTriangleDoc,
    NavmeshDoc, PartRole, PluginEntry, RawStore, RefFlags, RefGroup, RefKind, Reference,
    SCHEMA_VERSION, SkeletonInfo, Slice, TeleportDoc, TextureInfo, Transform, cell_of,
};
pub use paths::{
    FormKey, MESH_ROOT, TEXTURE_ROOT, creature_model_path, form_file_stem, normalize_mesh_path,
    normalize_texture_path,
};
pub use reader::{
    BadRecordRow, BaseObject, ContainerInfo, DoorInfo, Mir, NpcAppearance, PlacedRow,
    QuestStageRow, RecordRow, Records, ScriptRow,
};
pub use report::{ART_LIST_LIMIT, MAP_SQUARES, ReportParams, SCENERY_KINDS, report};
pub use source::DataSource;
