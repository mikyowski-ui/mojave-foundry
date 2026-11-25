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
//! [`storeys()`] splits a cell's navmeshes into levels by height, for
//! interior cells, which have no land; [`GroundSampler::flat`] is the
//! ground of such a cell.
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
//! - Since [`RECORDS_REVISION`] 1: `meta` also holds `game` and
//!   `records_revision`; `terminals`, `terminal_items`, `notes`,
//!   `note_quests`, `packages`, `actor_packages(actor, seq, package)`,
//!   `script_refs(owner, context, seq, form)` (every `SCRO` of scripts,
//!   quest stages, dialogue results, terminal items and package events),
//!   `quest_targets(quest, objective, seq, target, flags)`; `weapons` has
//!   the combat columns of `fnv_esm::WeaponCombat`; terminal items and
//!   packages add rows to `conditions` and `embedded_scripts`.
//! - Since [`RECORDS_REVISION`] 2 (the simulation tables, typed queries in
//!   `Records`): `globals`, `game_settings`, `messages` and
//!   `message_buttons`, `factions` and `faction_relations`, `actor_ai` and
//!   `actor_factions` (the AI data, voice type and factions of every NPC
//!   and creature, read from the template its flags name), `info_links`
//!   (`TCLT`, `TCLF`, `TCFU`, `NAME`), `package_events` (idle, topic and
//!   script of each event), `script_contexts`, `script_ref_list` and
//!   `script_locals` (every embedded script's header, whole reference list
//!   and variables, by owner and context), `decode_skips` (what a decoder
//!   could not use, counted by name); `infos` has the flag byte count,
//!   `KNAM` and the speech-challenge level, `packages` the behaviour and
//!   type-specific flags, `conditions` the resolved placed reference of a
//!   parameter (`param1_ref`, `param2_ref`).
//!
//! # Two games
//!
//! A plugin list of Fallout 3 or of New Vegas extracts the same way. The
//! game is detected from the first plugin ([`extract`]) or given
//! ([`extract_for_game`]), and recorded: [`Mir::game`],
//! [`Records::game`]. A directory written before the game was recorded
//! reads as New Vegas.
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
mod landscape;
mod logic;
mod model;
mod paths;
mod reader;
mod report;
mod simlogic;
mod source;
pub mod storeys;

pub use actors::{
    ARMED_FAMILIES, DEFAULT_FAMILIES, DEFAULT_WEAPONS, HUMAN_ANIMATIONS, HUMAN_SKELETON,
    HUMAN_STAND, HUMAN_WALK_FEMALE, HUMAN_WALK_MALE, MAX_CHAIN, USE_AI_DATA, USE_BASE_DATA,
    USE_FACTIONS, USE_INVENTORY, USE_MODEL, USE_STATS, USE_TRAITS, human_animation_slots,
    weapon_family,
};
pub use assets::AssetCounts;
pub use error::MirError;
pub use extract::{ExtractOptions, ExtractSummary, extract, extract_for_game};
pub use fnv_esm::GameVariant;
pub use ground::{GroundSampler, MAP_SIDE_SQUARES, MapWindow};
pub use index::content_hash;
pub use landscape::{DEFAULT_LAND_TEXTURE, DefaultLandTexture, default_land_texture};
pub use logic::{
    ConditionRow, NoteRow, PackageEventRow, PackageLocationRow, PackageRow, PackageTargetRow,
    QuestTargetRow, RefReason, Referral, TerminalItemRow, TerminalRow,
};
pub use model::{
    ActorAnimations, ActorDoc, ActorEntry, ActorKind, ActorPart, ActorTint, ActorWeapon,
    AnimationInfo, ArchiveEntry, AssetEntry, AssetStatus, CELL_UNITS, CellDoc, CellKey,
    CellLocation, CellRef, EnableParentDoc, Image, InteriorEntry, Issue, LAND_GRID, LAND_SPACING,
    LandDefault, LandDoc, LandLayer, LayerOpacity, LockDoc, Manifest, MeshInfo, NavDoorLinkDoc,
    NavExternalLinkDoc, NavTriangleDoc, NavmeshDoc, PartRole, PluginEntry, RECORDS_REVISION,
    RawStore, RefFlags, RefGroup, RefKind, Reference, SCHEMA_VERSION, SkeletonInfo, Slice,
    TeleportDoc, TextureInfo, Transform, cell_of,
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
pub use simlogic::{
    ActorAiRow, ActorFactionRow, DecodeSkipRow, FactionRelationRow, FactionRow, GameSettingRow,
    GlobalRow, InfoResponseRow, InfoRow, MessageButtonRow, MessageRow, ScriptContextRow,
    ScriptLocalRow, ScriptRefKind, ScriptRefRow,
};
pub use source::DataSource;
pub use storeys::{DEFAULT_STOREY_GAP, Storey, navmesh_storeys, storeys, storeys_with_gap};
