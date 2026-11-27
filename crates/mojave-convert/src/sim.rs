//! The simulation stage: `mojave-convert sim --out <dir> [--f2 <dir>]`
//! (`docs/simulation-layer-spec.md` section 10, task T9).
//!
//! It opens a finished output directory (the MIR, the built layer under
//! `mods/mojave`, the map stage's `map-placements.json` and `map-sim.json`),
//! builds the module (`transpile::build`), lowers it
//! (`transpile::lower_layer`) and writes the result into the layer through
//! `emit`: the `.int` programs under `scripts/`, the per-program dialogue
//! message files, the whole `scripts.lst` (owner decision 7), the appended
//! `vault13.gam`, `quests.txt`, `holodisk.txt` and shared message files, the
//! script records attached to each built map (`emit::map::attach_scripts`)
//! and the manifest's `[[global_scripts]]` and `[[hooks]]` rows. It writes
//! `sim-report.md` beside `map-report.md` (spec 10.5).
//!
//! # Inputs
//!
//! [`SimInputs::load`] reads everything once. The placed objects, the
//! markers (positions with no object) and the trigger volumes come from the
//! placement index; the walkable masks, the doors to other maps, the item
//! PIDs and which maps are interiors from `map-sim.json` (both written by
//! the map stage, `crate::placements`). The base copies of spec 10.1
//! ([`BASE_FILES`]) are the layer's own when the map stage shipped one, else
//! the Fallout 2 install's (`--f2`); with neither (`--report-only` with no
//! install) the lowering starts from empty tables.
//!
//! # Idempotence
//!
//! The stage keeps the map stage's own versions of every file it replaces
//! (the manifest, the maps, a base copy the layer shipped) under
//! `<out>/cache/sim/pristine/`, and in `<out>/cache/sim/state.json` the
//! hash of every file of the layer it left behind. On a run, a layer whose
//! files still hash as the state says holds this stage's output: the
//! pristine copies are its inputs, its own files are dropped and the maps
//! are attached afresh from the pristine ones (attaching never stacks). Any
//! other layer is taken as the map stage's fresh output and its files become
//! the pristine copies. Either way the inputs are the map stage's files, so
//! the key and the output do not depend on how many times the stage ran.
//!
//! # Cache
//!
//! Stage key `sim-1` ([`SimInputs::cache_key`], spec 10.3): the stage
//! version, the transpiler revision, the `[sim]` options and the layer's
//! `sim-options.toml` (see [`effective_config`]), the game, the
//! slice, the MIR's identity (its `manifest.json`, which carries the slice
//! and the plugins' SHA-256), the base files, the map stage's key, the
//! pristine manifest and maps, the placement index and the facts file. With
//! the stored key equal, the layer holding this stage's output and the
//! report present, the run is a cache hit and changes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mir::GameVariant;
use serde::{Deserialize, Serialize};
use transpile::diag::{Section, SectionSummary};
use transpile::ir::{
    BaseFiles, FormKey as IrKey, HexAt, Module, PlacedGroup, PlacedKind as IrKind, PlacedLock,
    PlacedObject, Placement, SimConfig, SimId, SimKind, SimSlice, WalkMask,
};
use transpile::lowered::{LowerFacts, LowerOutput, SectionCounts};
use vault_formats::mod_manifest::{MANIFEST, ModManifest};

use crate::map::LAYER_ID;
use crate::placements::{self, PlacedKind, PlacedRef, PlacementError, PlacementIndex, StageFacts};

/// The stage's cache key version (spec 10.3: `sim-1`).
pub const SIM_VERSION: &str = "sim-1";

/// The transpiler revision the key names. `transpile` exposes no version
/// of its own; this constant moves with every change of its output.
pub const TRANSPILER_REVISION: &str = "transpile pass-2 fix1 (2026-11-26)";

/// The report's name, beside `map-report.md`.
pub const REPORT_FILE: &str = "sim-report.md";

/// Where the stage keeps its state, under `--out`.
pub const STATE_DIR: &str = "cache/sim";

/// The stored key, under `--out`.
pub const KEY_FILE: &str = "cache/sim.key";

/// The base copies of spec 10.1, as layer paths.
pub const BASE_FILES: [&str; 7] = [
    "scripts/scripts.lst",
    "data/vault13.gam",
    "data/quests.txt",
    "data/holodisk.txt",
    "text/english/game/quests.msg",
    "text/english/game/pipboy.msg",
    "text/english/game/map.msg",
];

/// A failure of the stage's inputs or of writing its output (a failure of
/// the build itself is a build error of the report, not this).
#[derive(Debug, thiserror::Error)]
#[error("{what}: {message}")]
pub struct SimError {
    /// The file or step.
    pub what: String,
    /// What went wrong.
    pub message: String,
}

fn fail(what: impl Into<String>, message: impl ToString) -> SimError {
    SimError {
        what: what.into(),
        message: message.to_string(),
    }
}

impl From<PlacementError> for SimError {
    fn from(error: PlacementError) -> Self {
        fail(error.what, error.message)
    }
}

impl From<mir::MirError> for SimError {
    fn from(error: mir::MirError) -> Self {
        fail("mir", error)
    }
}

/// How the stage runs.
#[derive(Debug, Clone, Default)]
pub struct SimOptions {
    /// The Fallout 2 install, for the base copies the layer lacks.
    /// Required when the stage writes the layer.
    pub f2: Option<PathBuf>,
    /// Build and lower in memory and write only `sim-report.md`: no
    /// install needed, the layer and the cache untouched.
    pub report_only: bool,
    /// Run even when the stored key matches.
    pub force: bool,
    /// The `[sim]` table of `foundry.toml`, laid over the layer's
    /// `[sim_options]` ([`effective_config`]).
    pub config: SimConfig,
}

/// The build options of a run: the layer file's `[sim_options]` (left by
/// the map stage in `<out>/sim-options.toml`) with every key of `given`
/// (the `--config` file's `[sim]` table) that is not at its default laid
/// over it; `given` alone when the layer has no table. A `[sim]` key set to
/// its default value does not override the layer's. Also returns the
/// file's text, for the cache key.
///
/// # Errors
///
/// The file does not parse as `SimConfig` options.
pub fn effective_config(
    out: &Path,
    given: &SimConfig,
) -> Result<(SimConfig, Option<String>), SimError> {
    let path = out.join(crate::layer::SIM_OPTIONS_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok((given.clone(), None));
    };
    let file = crate::layer::SIM_OPTIONS_FILE;
    let mut table: toml::Table = toml::from_str(&text).map_err(|e| fail(file, e))?;
    let defaults = toml::Table::try_from(SimConfig::default()).map_err(|e| fail(file, e))?;
    let over = toml::Table::try_from(given.clone()).map_err(|e| fail(file, e))?;
    for (key, value) in over {
        if defaults.get(&key) != Some(&value) {
            table.insert(key, value);
        }
    }
    let config = toml::Value::Table(table)
        .try_into::<SimConfig>()
        .map_err(|e| fail(file, e))?;
    Ok((config, Some(text)))
}

fn hash(bytes: &[u8]) -> String {
    mir::content_hash(bytes)
}

fn digest(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    hash(text.as_bytes())
}

// ---- State and pristine copies ------------------------------------------------

/// What the last writing run left: its key and the hash of every layer
/// file it wrote or kept in place of the map stage's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    key: String,
    /// Layer path to the hash of its bytes after the run.
    written: BTreeMap<String, String>,
}

fn state_path(out: &Path) -> PathBuf {
    out.join(STATE_DIR).join("state.json")
}

fn pristine_dir(out: &Path) -> PathBuf {
    out.join(STATE_DIR).join("pristine")
}

fn read_state(out: &Path) -> Option<State> {
    let text = std::fs::read_to_string(state_path(out)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether the layer holds the output the state describes.
fn holds_output(layer_dir: &Path, state: &State) -> bool {
    !state.written.is_empty()
        && state.written.iter().all(|(path, digest)| {
            std::fs::read(layer_dir.join(path)).is_ok_and(|bytes| hash(&bytes) == *digest)
        })
}

/// The map stage's files this stage reads and may replace: the manifest
/// text, the registered maps, and any base copy or script file the layer
/// shipped before this stage ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pristine {
    /// `vault-mod.toml`.
    pub manifest: String,
    /// Layer path to bytes (`maps/*.map`, base copies, `scripts/*`,
    /// `text/english/dialog/*`).
    pub files: BTreeMap<String, Vec<u8>>,
}

fn layer_files_under(layer_dir: &Path, folder: &str, out: &mut BTreeMap<String, Vec<u8>>) {
    let mut stack = vec![layer_dir.join(folder)];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(layer_dir)
                && let Ok(bytes) = std::fs::read(&path)
            {
                let key = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
                    .to_ascii_lowercase();
                out.insert(key, bytes);
            }
        }
    }
}

impl Pristine {
    /// The map stage's files as the layer holds them now.
    fn from_layer(
        layer_dir: &Path,
        manifest_text: &str,
        manifest: &ModManifest,
    ) -> Result<Self, SimError> {
        let mut files = BTreeMap::new();
        for registration in &manifest.maps {
            let path = format!("maps/{}", registration.file);
            let bytes = std::fs::read(layer_dir.join(&path)).map_err(|e| fail(path.clone(), e))?;
            files.insert(path, bytes);
        }
        for path in BASE_FILES {
            if let Ok(bytes) = std::fs::read(layer_dir.join(path)) {
                files.insert(path.to_owned(), bytes);
            }
        }
        layer_files_under(layer_dir, "scripts", &mut files);
        layer_files_under(layer_dir, "text/english/dialog", &mut files);
        Ok(Self {
            manifest: manifest_text.to_owned(),
            files,
        })
    }

    /// The copies a writing run kept.
    fn read(out: &Path) -> Result<Self, SimError> {
        let dir = pristine_dir(out);
        let manifest = std::fs::read_to_string(dir.join(MANIFEST))
            .map_err(|e| fail(format!("{STATE_DIR}/pristine/{MANIFEST}"), e))?;
        let mut files = BTreeMap::new();
        layer_files_under(&dir, "", &mut files);
        files.remove(&MANIFEST.to_ascii_lowercase());
        Ok(Self { manifest, files })
    }

    fn write(&self, out: &Path) -> Result<(), SimError> {
        let dir = pristine_dir(out);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| fail(STATE_DIR, e))?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| fail(STATE_DIR, e))?;
        std::fs::write(dir.join(MANIFEST), &self.manifest).map_err(|e| fail(STATE_DIR, e))?;
        for (path, bytes) in &self.files {
            let target = dir.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| fail(STATE_DIR, e))?;
            }
            std::fs::write(&target, bytes).map_err(|e| fail(STATE_DIR, e))?;
        }
        Ok(())
    }
}

// ---- Inputs -------------------------------------------------------------------

/// Where one base copy came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseSource {
    /// The layer's own copy (the map stage shipped it).
    Layer,
    /// The Fallout 2 install.
    Install,
    /// Neither: the lowering starts from an empty table.
    Absent,
}

impl BaseSource {
    fn label(self) -> &'static str {
        match self {
            BaseSource::Layer => "the layer's own copy",
            BaseSource::Install => "the Fallout 2 install",
            BaseSource::Absent => "absent (empty table)",
        }
    }
}

/// Everything the stage reads, loaded once.
#[derive(Debug, Clone)]
pub struct SimInputs {
    /// The output directory.
    pub out: PathBuf,
    /// The source game.
    pub game: GameVariant,
    /// The layer's name.
    pub layer: String,
    /// The placement index, from the file or reconstructed.
    pub index: PlacementIndex,
    /// The index came from `map-placements.json`.
    pub from_file: bool,
    /// `map-sim.json`, when the map stage wrote it.
    pub facts: Option<StageFacts>,
    /// The map stage's files.
    pub pristine: Pristine,
    /// The layer holds this stage's earlier output.
    pub has_output: bool,
    /// The base copies, and where each came from.
    pub base: BaseFiles,
    pub base_sources: Vec<(&'static str, BaseSource)>,
    /// The build's options.
    pub config: SimConfig,
    /// The slice the build compiles.
    pub slice: SimSlice,
    /// Lines the run prints and the report keeps.
    pub warnings: Vec<String>,
    /// What the cache key is made of, in order.
    pub key_parts: Vec<(String, String)>,
}

fn ir_kind(kind: PlacedKind) -> IrKind {
    match kind {
        PlacedKind::Critter => IrKind::Critter,
        PlacedKind::Door => IrKind::Door,
        PlacedKind::Container => IrKind::Container,
        PlacedKind::Item => IrKind::Item,
        PlacedKind::Terminal => IrKind::Terminal,
        PlacedKind::Scenery => IrKind::Scenery,
    }
}

/// One placed entry of the index as the build takes it: a presence copy
/// names the copied actor's own reference with `presence` set.
pub fn placed_object(p: &PlacedRef) -> PlacedObject {
    let reference = p.presence_of.clone().or_else(|| p.reference.clone());
    PlacedObject {
        map: p.map.clone(),
        map_number: p.map_number,
        elevation: p.elevation,
        hex: p.hex.max(0) as u32,
        kind: ir_kind(p.kind),
        object_id: p.object_id,
        pid: p.pid,
        reference: reference.map(IrKey),
        base: p.base.clone().map(IrKey),
        hidden: p.hidden,
        group: p.group.as_ref().map(|g| PlacedGroup {
            root: IrKey(g.root.clone()),
            opposite: g.opposite,
            initially_enabled: g.initially_enabled,
        }),
        lock: p.lock.as_ref().map(|l| PlacedLock {
            locked: l.locked,
            level: l.level,
            key: l.key.clone().map(IrKey),
            key_pid: l.key_pid,
        }),
        linked_ref: p.linked_ref.clone().map(IrKey),
        hostile: p.hostile,
        presence: p.presence_of.is_some(),
        marker: false,
    }
}

/// The build's slice: every placed entry, every marker (no object, its
/// projected hex), the walkable masks and the exits.
pub fn sim_slice(index: &PlacementIndex, facts: Option<&StageFacts>) -> Result<SimSlice, SimError> {
    let mut objects: Vec<PlacedObject> = index.placed.iter().map(placed_object).collect();
    for marker in &index.markers {
        objects.push(PlacedObject {
            map: marker.map.clone(),
            map_number: marker.map_number,
            elevation: marker.elevation,
            hex: marker.hex.max(0) as u32,
            kind: IrKind::Scenery,
            object_id: None,
            pid: None,
            reference: Some(IrKey(marker.reference.clone())),
            base: marker.base.clone().map(IrKey),
            marker: true,
            ..PlacedObject::default()
        });
    }
    let mut placement = Placement { objects };
    placement.sort();
    let mut walkable: BTreeMap<String, Vec<WalkMask>> = BTreeMap::new();
    let mut exits: BTreeMap<String, Vec<(IrKey, HexAt, String)>> = BTreeMap::new();
    for map in facts.map(|f| f.maps.as_slice()).unwrap_or_default() {
        let mut masks = Vec::new();
        for runs in &map.walkable {
            masks.push(WalkMask {
                elevation: runs.elevation,
                hexes: runs.hexes()?,
            });
        }
        walkable.insert(map.stem.clone(), masks);
        let list: Vec<(IrKey, HexAt, String)> = map
            .exits
            .iter()
            .map(|e| {
                (
                    IrKey(e.door.clone()),
                    HexAt {
                        elevation: e.elevation,
                        hex: e.hex.max(0) as u32,
                    },
                    e.to.clone(),
                )
            })
            .collect();
        if !list.is_empty() {
            exits.insert(map.stem.clone(), list);
        }
    }
    Ok(SimSlice {
        name: index.layer.clone(),
        placement,
        walkable,
        exits,
    })
}

/// The base files, where each came from, and their key parts.
type BaseRead = (
    BaseFiles,
    Vec<(&'static str, BaseSource)>,
    Vec<(String, String)>,
);

/// The base copies: the layer's own when the map stage shipped one, else
/// the install's, else none.
fn base_files(
    pristine: &Pristine,
    f2: Option<&Path>,
    warnings: &mut Vec<String>,
) -> Result<BaseRead, SimError> {
    let master = match f2 {
        Some(dir) => Some(crate::config::f2_master(dir).map_err(|e| fail("--f2", e))?),
        None => None,
    };
    let mut texts: Vec<Option<String>> = Vec::new();
    let mut sources = Vec::new();
    let mut key = Vec::new();
    for path in BASE_FILES {
        let (bytes, source) = if let Some(bytes) = pristine.files.get(path) {
            (Some(bytes.clone()), BaseSource::Layer)
        } else if let (Some(dir), Some(master)) = (f2, &master) {
            match crate::config::f2_file_in(dir, master.as_deref(), &path.replace('/', "\\")) {
                Ok(bytes) => (Some(bytes), BaseSource::Install),
                Err(_) => {
                    warnings.push(format!(
                        "base file {path}: not in the Fallout 2 install; the lowering starts it \
                         empty"
                    ));
                    (None, BaseSource::Absent)
                }
            }
        } else {
            (None, BaseSource::Absent)
        };
        key.push((
            format!("base file {path}"),
            bytes.as_deref().map_or_else(|| "absent".to_owned(), hash),
        ));
        let text = bytes.map(|b| match String::from_utf8(b) {
            Ok(text) => text,
            Err(e) => {
                warnings.push(format!(
                    "base file {path} is not UTF-8; read with replacement characters"
                ));
                String::from_utf8_lossy(e.as_bytes()).into_owned()
            }
        });
        texts.push(text);
        sources.push((path, source));
    }
    let mut it = texts.into_iter();
    let mut next = || it.next().flatten();
    let base = BaseFiles {
        scripts_lst: next(),
        vault13_gam: next(),
        quests_txt: next(),
        holodisk_txt: next(),
        quests_msg: next(),
        pipboy_msg: next(),
        map_msg: next(),
    };
    Ok((base, sources, key))
}

impl SimInputs {
    /// Opens an output directory and reads everything the stage uses.
    ///
    /// # Errors
    ///
    /// The MIR or the layer is missing or does not open, the index belongs
    /// to another game or names a map the layer lacks, or a base copy of
    /// the install cannot be read.
    pub fn load(out: &Path, options: &SimOptions) -> Result<Self, SimError> {
        let mir = mir::Mir::open(&out.join("mir")).map_err(|e| {
            fail(
                "mir",
                format!("{e}; run extract, or point --out at a finished run"),
            )
        })?;
        let game = mir.game();
        let layer_dir = out.join("mods").join(LAYER_ID);
        let manifest_text = std::fs::read_to_string(layer_dir.join(MANIFEST))
            .map_err(|e| fail(format!("mods/{LAYER_ID}/{MANIFEST}"), e))?;
        let manifest = ModManifest::parse(&manifest_text)
            .map_err(|e| fail(format!("mods/{LAYER_ID}/{MANIFEST}"), e))?;
        let mut warnings = Vec::new();

        let state = read_state(out);
        let has_output = state.as_ref().is_some_and(|s| holds_output(&layer_dir, s));
        let pristine = if has_output {
            Pristine::read(out)?
        } else {
            Pristine::from_layer(&layer_dir, &manifest_text, &manifest)?
        };
        let pristine_manifest = ModManifest::parse(&pristine.manifest)
            .map_err(|e| fail(format!("{STATE_DIR}/pristine/{MANIFEST}"), e))?;

        let (index, from_file) = match placements::read(out)? {
            Some(index) => (index, true),
            None => {
                warnings.push(format!(
                    "{} not found: reconstructed the placement index from the built maps (no \
                     references, so nothing is attached); run the map stage for the full layer",
                    placements::FILE_NAME
                ));
                (
                    placements::reconstruct(&layer_dir, Some(out), game.id())?,
                    false,
                )
            }
        };
        if index.game != game.id() {
            return Err(fail(
                placements::FILE_NAME,
                format!(
                    "the index is for {} but the MIR is {}; the output directory mixes two runs",
                    index.game,
                    game.id()
                ),
            ));
        }
        for entry in &index.maps {
            let registered = pristine_manifest
                .maps
                .iter()
                .any(|m| m.file == format!("{}.map", entry.stem) && m.number == entry.number);
            if !registered {
                return Err(fail(
                    placements::FILE_NAME,
                    format!(
                        "map {} ({}) is not in the layer's manifest; the index is from another \
                         run",
                        entry.stem, entry.number
                    ),
                ));
            }
        }
        let facts = placements::read_facts(out)?;
        if facts.is_none() {
            warnings.push(format!(
                "{} not found (a map stage from before task T9): no walkable masks, exits or \
                 item PIDs; run the map stage again",
                placements::FACTS_FILE
            ));
        }
        if from_file && index.markers.is_empty() && facts.is_none() {
            warnings.push(
                "the placement index has no markers: package locations on markers have no hex"
                    .to_owned(),
            );
        }
        let slice = sim_slice(&index, facts.as_ref())?;

        let (base, base_sources, base_key) =
            base_files(&pristine, options.f2.as_deref(), &mut warnings)?;

        let (config, layer_options) = effective_config(out, &options.config)?;
        let read_or =
            |path: PathBuf| std::fs::read(path).map_or_else(|_| "absent".to_owned(), |b| hash(&b));
        let mut key_parts: Vec<(String, String)> = vec![
            ("sim".into(), SIM_VERSION.into()),
            ("transpiler".into(), TRANSPILER_REVISION.into()),
            (
                "foundry.toml [sim]".into(),
                serde_json::to_string(&options.config).unwrap_or_default(),
            ),
            (
                "layer [sim_options]".into(),
                layer_options.map_or_else(|| "absent".to_owned(), |t| hash(t.as_bytes())),
            ),
            (
                "sim options".into(),
                serde_json::to_string(&config).unwrap_or_default(),
            ),
            ("game".into(), game.id().into()),
            (
                "slice".into(),
                hash(
                    serde_json::to_string(&mir.manifest().slice)
                        .unwrap_or_default()
                        .as_bytes(),
                ),
            ),
            (
                "mir manifest".into(),
                read_or(out.join("mir").join("manifest.json")),
            ),
        ];
        key_parts.extend(base_key);
        let map_key = std::fs::read_to_string(out.join("cache").join("map.key"))
            .map_or_else(|_| "absent".to_owned(), |t| t.trim().to_owned());
        key_parts.push(("map stage key".into(), map_key));
        key_parts.push((
            "pristine manifest".into(),
            hash(pristine.manifest.as_bytes()),
        ));
        for (path, bytes) in &pristine.files {
            key_parts.push((format!("pristine {path}"), hash(bytes)));
        }
        key_parts.push(("placement index".into(), hash(index.to_text().as_bytes())));
        key_parts.push((
            "map facts".into(),
            facts
                .as_ref()
                .map_or_else(|| "absent".to_owned(), |f| hash(f.to_text().as_bytes())),
        ));

        Ok(Self {
            out: out.to_owned(),
            game,
            layer: index.layer.clone(),
            index,
            from_file,
            facts,
            pristine,
            has_output,
            base,
            base_sources,
            config,
            slice,
            warnings,
            key_parts,
        })
    }

    /// The stage cache key (spec 10.3): `sim-1:` and the content hash of
    /// every part, named, in order.
    pub fn cache_key(&self) -> String {
        let lines: Vec<String> = self
            .key_parts
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        format!("{SIM_VERSION}:{}", digest(&lines))
    }

    /// The facts the lowering reads beyond the module.
    fn lower_facts(&self, source: &transpile::source::SimSource) -> LowerFacts {
        let mut facts = LowerFacts::of_slice(&self.slice);
        if let Some(stage) = &self.facts {
            for (item, pid) in &stage.items {
                facts.pids.insert(IrKey(item.clone()), *pid);
            }
            facts.interiors = stage
                .maps
                .iter()
                .filter(|m| m.interior)
                .map(|m| m.stem.clone())
                .collect();
        }
        for entry in &self.index.placed {
            if let Some(lock) = &entry.lock
                && let (Some(key), Some(pid)) = (&lock.key, lock.key_pid)
            {
                facts.pids.insert(IrKey(key.clone()), pid);
            }
        }
        for object in &self.slice.placement.objects {
            if object.kind == IrKind::Critter || object.marker {
                continue;
            }
            if let Some(base) = &object.base
                && let Some(script) = source.base_scripts.get(base)
            {
                facts
                    .base_scripts
                    .insert(base.clone(), SimId::of(SimKind::Script, script));
            }
        }
        facts
    }
}

// ---- The run ------------------------------------------------------------------

/// What the build and the lowering made, or why they did not.
pub struct Built {
    /// The module, when the build ran.
    pub module: Option<Module>,
    /// The lowering's output, when it ran.
    pub lowered: Option<LowerOutput>,
    /// The hash of the canonical JSON of what the build read (the MIR rows).
    pub source_hash: Option<String>,
    /// Build errors (a failed build or lowering, a panic), with the step.
    pub errors: Vec<String>,
}

fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

/// Builds and lowers in memory. Never fails: a failure is a build error.
pub fn build(inputs: &SimInputs) -> Built {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let mut built = Built {
        module: None,
        lowered: None,
        source_hash: None,
        errors: Vec::new(),
    };
    let mir = match mir::Mir::open(&inputs.out.join("mir")) {
        Ok(mir) => mir,
        Err(e) => {
            built.errors.push(format!("open the MIR: {e}"));
            return built;
        }
    };
    let source = {
        let _t = crate::timings::span("read the source (transpile::source::load)");
        match catch_unwind(AssertUnwindSafe(|| {
            transpile::source::load(&mir, &inputs.slice)
        })) {
            Ok(Ok(source)) => source,
            Ok(Err(e)) => {
                built.errors.push(format!("transpile::source::load: {e}"));
                return built;
            }
            Err(p) => {
                built.errors.push(format!(
                    "transpile::source::load panicked: {}",
                    panic_text(p)
                ));
                return built;
            }
        }
    };
    built.source_hash = Some(hash(
        serde_json::to_string(&source)
            .unwrap_or_default()
            .as_bytes(),
    ));
    let module = {
        let _t = crate::timings::span("build the module (transpile::build_module)");
        match catch_unwind(AssertUnwindSafe(|| {
            transpile::build_module(&source, &inputs.slice, &inputs.config)
        })) {
            Ok(Ok(module)) => module,
            Ok(Err(e)) => {
                built.errors.push(format!("transpile::build_module: {e}"));
                return built;
            }
            Err(p) => {
                built.errors.push(format!(
                    "transpile::build_module panicked: {}",
                    panic_text(p)
                ));
                return built;
            }
        }
    };
    let facts = inputs.lower_facts(&source);
    let lowered = {
        let _t = crate::timings::span("lower (transpile::lower_layer)");
        lower(&module, &facts, &inputs.base, &inputs.config)
    };
    match lowered {
        Ok(out) => built.lowered = Some(out),
        Err(e) => built.errors.push(e),
    }
    built.module = Some(module);
    built
}

/// The one call into the lowering (its signature may still move).
fn lower(
    module: &Module,
    facts: &LowerFacts,
    base: &BaseFiles,
    config: &SimConfig,
) -> Result<LowerOutput, String> {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    match catch_unwind(AssertUnwindSafe(|| {
        transpile::lower_layer(module, facts, base, config)
    })) {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("transpile::lower_layer: {e}")),
        Err(p) => Err(format!(
            "transpile::lower_layer panicked: {}",
            panic_text(p)
        )),
    }
}

/// What one map got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapAttached {
    /// The map's stem.
    pub map: String,
    /// The map script's row, when it has one.
    pub map_script: Option<u32>,
    /// Object script records attached.
    pub objects: usize,
    /// Spatial records attached.
    pub spatial: usize,
}

/// Writes the lowering's output into the layer through `emit`. Returns
/// what each map got and the new state.
fn write_layer(
    inputs: &SimInputs,
    out: &LowerOutput,
) -> Result<(Vec<MapAttached>, State), SimError> {
    let layer_dir = inputs.out.join("mods").join(LAYER_ID);
    let current = {
        let _t = crate::timings::span("read the layer");
        map_builder::read_layer(&layer_dir).map_err(|e| fail("read the layer", e))?
    };
    let manifest = ModManifest::parse(&inputs.pristine.manifest)
        .map_err(|e| fail(format!("{STATE_DIR}/pristine/{MANIFEST}"), e))?;
    let mut layer = emit::ModLayer::from_manifest(manifest).map_err(|e| fail(MANIFEST, e))?;
    // The map stage's files: the layer's, without this stage's earlier
    // output, with the pristine copies back in place.
    let previous: BTreeSet<String> = if inputs.has_output {
        read_state(&inputs.out)
            .map(|s| s.written.into_keys().collect())
            .unwrap_or_default()
    } else {
        BTreeSet::new()
    };
    for (path, bytes) in current.files() {
        if previous.contains(path) || inputs.pristine.files.contains_key(path) {
            continue;
        }
        layer
            .insert(path, bytes.clone())
            .map_err(|e| fail(path.clone(), e))?;
    }
    for (path, bytes) in &inputs.pristine.files {
        layer
            .insert(path, bytes.clone())
            .map_err(|e| fail(path.clone(), e))?;
    }
    // The lowering's files.
    for (path, bytes) in &out.lowered.files {
        layer
            .insert(path, bytes.clone())
            .map_err(|e| fail(path.clone(), e))?;
    }
    // Attach the scripts to the pristine maps.
    let mut attached = Vec::new();
    {
        let _t = crate::timings::span("attach scripts to the maps");
        let protos: Vec<(String, Vec<u8>)> = layer
            .files()
            .iter()
            .filter(|(p, _)| p.starts_with("proto/"))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        let catalog_files = proto_tree(protos)?;
        let catalog = vault_formats::pro_catalog::LocalPrototypeCatalog::for_game(
            &catalog_files,
            vault_formats::GameProfile::Fallout2,
        );
        for plan in &out.plan.maps {
            if plan.attachments.is_empty() && plan.map_script.is_none() {
                continue;
            }
            let path = format!("maps/{}.map", plan.map);
            let Some(bytes) = inputs.pristine.files.get(&path) else {
                return Err(fail(
                    path,
                    "the lowering names a map the layer does not hold",
                ));
            };
            let new =
                emit::map::attach_scripts(bytes, &catalog, &plan.attachments, plan.map_script)
                    .map_err(|e| fail(path.clone(), e))?;
            layer
                .insert(&path, new)
                .map_err(|e| fail(path.clone(), e))?;
            attached.push(MapAttached {
                map: plan.map.clone(),
                map_script: plan.map_script,
                objects: plan
                    .attachments
                    .iter()
                    .filter(|a| matches!(a.site, emit::map::ScriptSite::Object { .. }))
                    .count(),
                spatial: plan
                    .attachments
                    .iter()
                    .filter(|a| matches!(a.site, emit::map::ScriptSite::Spatial { .. }))
                    .count(),
            });
        }
    }
    for row in &out.plan.global_scripts {
        layer
            .add_global_script(&row.file, row.repeat, row.kind)
            .map_err(|e| fail(MANIFEST, e))?;
    }
    for row in &out.plan.hooks {
        layer
            .add_hook(&row.point, &row.file, &row.procedure)
            .map_err(|e| fail(MANIFEST, e))?;
    }
    {
        let _t = crate::timings::span("write the layer");
        layer
            .write(&layer_dir)
            .map_err(|e| fail("write the layer", e))?;
    }
    // The state: every file this stage wrote or replaced, as it is now.
    let mut written = BTreeMap::new();
    let manifest_text = layer.manifest_text().map_err(|e| fail(MANIFEST, e))?;
    written.insert(MANIFEST.to_owned(), hash(manifest_text.as_bytes()));
    // The README is made from the manifest (it lists the scripts too).
    let readme = vault_formats::mod_manifest::README;
    if let Ok(bytes) = std::fs::read(layer_dir.join(readme)) {
        written.insert(readme.to_owned(), hash(&bytes));
    }
    for (path, bytes) in layer.files() {
        let ours = out.lowered.files.contains_key(path) || inputs.pristine.files.contains_key(path);
        if ours {
            written.insert(path.clone(), hash(bytes));
        }
    }
    Ok((
        attached,
        State {
            key: inputs.cache_key(),
            written,
        },
    ))
}

/// The prototypes of the layer as a read-only file tree.
fn proto_tree(files: Vec<(String, Vec<u8>)>) -> Result<impl vault_formats::Vfs, SimError> {
    use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};
    let entries = files
        .into_iter()
        .map(|(path, bytes)| Ok((path, ByteSource::new(bytes).map_err(|e| fail("proto", e))?)))
        .collect::<Result<Vec<_>, SimError>>()?;
    let tree = MemoryDir::new(entries).map_err(|e| fail("proto", e))?;
    MemoryMount {
        profile: vault_formats::GameProfile::Fallout2,
        loose: Some(tree),
        ..MemoryMount::default()
    }
    .mount()
    .map_err(|e| fail("proto", e))
}

// ---- The report ---------------------------------------------------------------

/// The stage's result, for the report and the console.
#[derive(Debug, Clone)]
pub struct SimReport {
    /// The source game's name and id, the layer.
    pub game: String,
    pub game_id: String,
    pub layer: String,
    /// The run wrote nothing in the layer (`--report-only`).
    pub report_only: bool,
    /// Warnings of the load.
    pub warnings: Vec<String>,
    /// Build errors.
    pub errors: Vec<String>,
    /// Programs by kind, and in all.
    pub programs: BTreeMap<String, usize>,
    pub program_count: usize,
    /// Files of the lowering that are not programs: path and bytes.
    pub tables: Vec<(String, usize)>,
    /// The lowering's summary line.
    pub summary: Option<transpile::lowered::Summary>,
    /// The build's per-section summary (`transpile::diag::summarize`).
    pub build_sections: Vec<SectionSummary>,
    /// The lowering's per-section counts.
    pub lower_sections: Vec<(&'static str, SectionCounts)>,
    /// What each map got (attachments planned when nothing was written).
    pub maps: Vec<MapAttached>,
    /// `[[global_scripts]]` and `[[hooks]]` rows.
    pub manifest_rows: Vec<String>,
    /// Engine gaps the build ran into: letter, count, an example.
    pub gaps: Vec<(char, usize, String)>,
    /// Inputs: placed entries, markers, triggers; maps with walk data.
    pub placed: usize,
    pub markers: usize,
    pub triggers: usize,
    pub walk_maps: usize,
    pub exits: usize,
    /// Where the base copies came from.
    pub base_sources: Vec<(&'static str, BaseSource)>,
    /// The hash of what the build read.
    pub source_hash: Option<String>,
    /// The stage cache key and its parts.
    pub cache_key: String,
    pub key_parts: Vec<(String, String)>,
}

/// The engine gaps of spec 11 the module and the lowering run into: the
/// `gap X` a finding names, and the structures that always need one.
fn gaps_of(module: Option<&Module>, lowered: Option<&LowerOutput>) -> Vec<(char, usize, String)> {
    let mut found: BTreeMap<char, (usize, String)> = BTreeMap::new();
    let mut add = |letter: char, what: String| {
        let entry = found.entry(letter).or_insert((0, what));
        entry.0 += 1;
    };
    let messages = module.into_iter().flat_map(|m| m.diagnostics.iter()).chain(
        lowered
            .into_iter()
            .flat_map(|l| l.lowered.diagnostics.iter()),
    );
    for diag in messages {
        let text = &diag.message;
        let mut rest = text.as_str();
        while let Some(at) = rest.find("gap ") {
            let tail = &rest[at + 4..];
            if let Some(letter) = tail.chars().next()
                && letter.is_ascii_uppercase()
                && !tail[1..].starts_with(|c: char| c.is_ascii_alphanumeric())
            {
                add(letter, format!("{}: {}", diag.category.name(), text));
            }
            rest = tail;
        }
    }
    if let Some(l) = lowered {
        if !l.plan.programs.is_empty() {
            add(
                'A',
                "every program is assembled by emit's local adapter".into(),
            );
        }
        if l.lowered.files.contains_key("scripts/scripts.lst") {
            add('S', "a whole scripts.lst ships (owner decision 7)".into());
        }
    }
    if let Some(m) = module {
        for _ in m.terminal_programs.values() {
            add('T', "terminals are no-head dialogues".into());
        }
        for run in m.runs.values() {
            if !run.presence.is_empty() {
                add(
                    'P',
                    "presence copies stand in for critters crossing maps".into(),
                );
            }
        }
        for _ in m.groups.values() {
            add(
                'U',
                "group changes from code outside dialogue show up to 60 s late".into(),
            );
        }
        for q in m.quests.values() {
            if q.objectives.iter().any(|o| !o.targets.is_empty()) {
                add('O', "objective targets are kept, not shown".into());
            }
        }
    }
    found
        .into_iter()
        .map(|(letter, (count, what))| (letter, count, what))
        .collect()
}

fn lower_sections(out: &LowerOutput) -> Vec<(&'static str, SectionCounts)> {
    let c = &out.plan.counts;
    vec![
        ("dialogue", c.dialogue.clone()),
        ("scripts", c.scripts.clone()),
        ("quests", c.quests.clone()),
        ("state", c.state.clone()),
        ("terminals", c.terminals.clone()),
        ("packages", c.packages.clone()),
    ]
}

impl SimReport {
    fn new(inputs: &SimInputs, built: &Built, maps: Vec<MapAttached>, report_only: bool) -> Self {
        let mut programs: BTreeMap<String, usize> = BTreeMap::new();
        let mut tables = Vec::new();
        let mut manifest_rows = Vec::new();
        let mut maps = maps;
        if let Some(out) = &built.lowered {
            for p in &out.plan.programs {
                *programs.entry(p.kind.to_owned()).or_default() += 1;
            }
            for (path, bytes) in &out.lowered.files {
                if !path.ends_with(".int") {
                    tables.push((path.clone(), bytes.len()));
                }
            }
            for row in &out.plan.global_scripts {
                manifest_rows.push(format!(
                    "[[global_scripts]] file = {}, repeat = {}, type = {}",
                    row.file, row.repeat, row.kind
                ));
            }
            for row in &out.plan.hooks {
                manifest_rows.push(format!(
                    "[[hooks]] point = {}, file = {}, procedure = {}",
                    row.point, row.file, row.procedure
                ));
            }
            if maps.is_empty() {
                maps = out
                    .plan
                    .maps
                    .iter()
                    .filter(|p| !p.attachments.is_empty() || p.map_script.is_some())
                    .map(|p| MapAttached {
                        map: p.map.clone(),
                        map_script: p.map_script,
                        objects: p
                            .attachments
                            .iter()
                            .filter(|a| matches!(a.site, emit::map::ScriptSite::Object { .. }))
                            .count(),
                        spatial: p
                            .attachments
                            .iter()
                            .filter(|a| matches!(a.site, emit::map::ScriptSite::Spatial { .. }))
                            .count(),
                    })
                    .collect();
            }
        }
        Self {
            game: inputs.game.display_name().to_owned(),
            game_id: inputs.game.id().to_owned(),
            layer: inputs.layer.clone(),
            report_only,
            warnings: inputs.warnings.clone(),
            errors: built.errors.clone(),
            program_count: programs.values().sum(),
            programs,
            tables,
            summary: built.lowered.as_ref().map(|l| l.plan.summary),
            build_sections: built
                .module
                .as_ref()
                .map(transpile::diag::summarize)
                .unwrap_or_default(),
            lower_sections: built
                .lowered
                .as_ref()
                .map(lower_sections)
                .unwrap_or_default(),
            maps,
            manifest_rows,
            gaps: gaps_of(built.module.as_ref(), built.lowered.as_ref()),
            placed: inputs.index.placed.len(),
            markers: inputs.index.markers.len(),
            triggers: inputs.index.triggers.len(),
            walk_maps: inputs.slice.walkable.len(),
            exits: inputs.slice.exits.values().map(Vec::len).sum(),
            base_sources: inputs.base_sources.clone(),
            source_hash: built.source_hash.clone(),
            cache_key: inputs.cache_key(),
            key_parts: inputs.key_parts.clone(),
        }
    }

    /// The counts of the lowering's section `name`.
    pub fn section(&self, name: &str) -> Option<&SectionCounts> {
        self.lower_sections
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, c)| c)
    }

    /// The failure the command exits with: `Some` when the build or the
    /// lowering failed (the report still lists the errors), `None` for a
    /// run with no build error. Counted fallbacks are not errors.
    pub fn failure(&self) -> Option<String> {
        if self.errors.is_empty() {
            return None;
        }
        Some(format!(
            "the simulation stage had {} build error{} (details in {REPORT_FILE})",
            self.errors.len(),
            if self.errors.len() == 1 { "" } else { "s" }
        ))
    }

    /// The lines the command prints.
    pub fn console_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("game: {} ({})", self.game, self.game_id),
            format!("layer: {}", self.layer),
            format!(
                "inputs: {} placed, {} markers, {} triggers; walk data for {} maps, {} exits",
                self.placed, self.markers, self.triggers, self.walk_maps, self.exits
            ),
        ];
        let kinds: Vec<String> = self
            .programs
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        lines.push(format!(
            "programs: {} ({})",
            self.program_count,
            kinds.join(", ")
        ));
        for (name, c) in &self.lower_sections {
            lines.push(format!(
                "{name}: compiled {}, folded {}, dropped {}, fallback {}",
                c.compiled, c.folded, c.dropped, c.fallback
            ));
        }
        for map in &self.maps {
            lines.push(format!(
                "map {}: map script {}, {} object scripts, {} spatial",
                map.map,
                map.map_script
                    .map_or_else(|| "none".to_owned(), |r| format!("row {r}")),
                map.objects,
                map.spatial
            ));
        }
        for error in &self.errors {
            lines.push(format!("build error: {error}"));
        }
        lines.push(format!("cache key: {}", self.cache_key));
        lines
    }

    /// The text of `sim-report.md`: no time, host or path.
    pub fn to_markdown(&self) -> String {
        let mut t = String::new();
        let _ = writeln!(t, "# Simulation layer report\n");
        let _ = writeln!(t, "## Summary\n");
        let _ = writeln!(t, "- Game: {} (`{}`)", self.game, self.game_id);
        let _ = writeln!(t, "- Layer: {}", self.layer);
        let _ = writeln!(
            t,
            "- Mode: {}",
            if self.report_only {
                "report only (built and lowered in memory; the layer is untouched)"
            } else {
                "written into the layer"
            }
        );
        let kinds: Vec<String> = self
            .programs
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        let _ = writeln!(
            t,
            "- Programs (`.int`): {}{}",
            self.program_count,
            if kinds.is_empty() {
                String::new()
            } else {
                format!(" ({})", kinds.join(", "))
            }
        );
        if let Some(s) = &self.summary {
            let _ = writeln!(
                t,
                "- Script rows {}, globals {}, messages {}, holodisks {}, quest rows {}, text \
                 changes {}",
                s.rows, s.globals, s.messages, s.holodisks, s.quest_rows, s.text_changes
            );
        }
        let _ = writeln!(
            t,
            "- Inputs: {} placed entries, {} markers, {} trigger volumes; walkable masks for {} \
             maps; {} exits",
            self.placed, self.markers, self.triggers, self.walk_maps, self.exits
        );
        let _ = writeln!(t, "- Build errors: {}", self.errors.len());
        let _ = writeln!(t, "- Stage cache key: `{}`", self.cache_key);

        if !self.errors.is_empty() {
            let _ = writeln!(t, "\n## Build errors\n");
            for error in &self.errors {
                let _ = writeln!(t, "- {error}");
            }
        }
        if !self.warnings.is_empty() {
            let _ = writeln!(t, "\n## Warnings\n");
            for warning in &self.warnings {
                let _ = writeln!(t, "- {warning}");
            }
        }

        let _ = writeln!(t, "\n## Sections (the lowering)\n");
        let _ = writeln!(
            t,
            "| section | compiled | folded | dropped | fallback |\n|---|---:|---:|---:|---:|"
        );
        for (name, c) in &self.lower_sections {
            let _ = writeln!(
                t,
                "| {name} | {} | {} | {} | {} |",
                c.compiled, c.folded, c.dropped, c.fallback
            );
        }
        for (name, c) in &self.lower_sections {
            if c.reasons.is_empty() {
                continue;
            }
            let _ = writeln!(t, "\n### {name}: reasons\n");
            let mut reasons: Vec<_> = c.reasons.iter().collect();
            reasons.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
            for (reason, r) in reasons.into_iter().take(12) {
                let _ = writeln!(
                    t,
                    "- {} × {} (first: {})",
                    r.count,
                    if reason.is_empty() {
                        "(none given)"
                    } else {
                        reason
                    },
                    r.owners.join(", ")
                );
            }
        }

        let _ = writeln!(
            t,
            "\n## Sections (the build, `transpile::diag::summarize`)\n"
        );
        let _ = writeln!(
            t,
            "| section | compiled | folded | dropped | fallback | notes |\n|---|---|---:|---:|---:|---:|"
        );
        for s in &self.build_sections {
            let Some(section) = s.section else { continue };
            let compiled: Vec<String> =
                s.compiled.iter().map(|(k, v)| format!("{k} {v}")).collect();
            let _ = writeln!(
                t,
                "| {} | {} | {} | {} | {} | {} |",
                section.name(),
                compiled.join(", "),
                s.folded,
                s.dropped,
                s.fallback,
                s.notes
            );
        }
        for s in &self.build_sections {
            let Some(section) = s.section else { continue };
            let top = s.top_reasons(8);
            if top.is_empty() {
                continue;
            }
            let _ = writeln!(t, "\n### {}: top reasons\n", section_title(section));
            for (reason, count) in top {
                let _ = writeln!(t, "- {count} × {reason}");
            }
        }

        let _ = writeln!(t, "\n## Tables written\n");
        if self.tables.is_empty() {
            let _ = writeln!(t, "None.");
        } else {
            let _ = writeln!(t, "| file | bytes |\n|---|---:|");
            for (path, bytes) in &self.tables {
                let _ = writeln!(t, "| `{path}` | {bytes} |");
            }
        }
        let _ = writeln!(t, "\n| base copy | from |\n|---|---|");
        for (path, source) in &self.base_sources {
            let _ = writeln!(t, "| `{path}` | {} |", source.label());
        }

        let _ = writeln!(t, "\n## Attachments per map\n");
        if self.maps.is_empty() {
            let _ = writeln!(t, "None.");
        } else {
            let _ = writeln!(
                t,
                "| map | map script row | object scripts | spatial |\n|---|---:|---:|---:|"
            );
            for map in &self.maps {
                let _ = writeln!(
                    t,
                    "| {} | {} | {} | {} |",
                    map.map,
                    map.map_script
                        .map_or_else(|| "none".to_owned(), |r| r.to_string()),
                    map.objects,
                    map.spatial
                );
            }
        }
        if !self.manifest_rows.is_empty() {
            let _ = writeln!(t, "\nManifest rows:\n");
            for row in &self.manifest_rows {
                let _ = writeln!(t, "- `{row}`");
            }
        }

        let _ = writeln!(t, "\n## Engine gaps hit\n");
        if self.gaps.is_empty() {
            let _ = writeln!(t, "None.");
        } else {
            let _ = writeln!(t, "| gap | count | for example |\n|---|---:|---|");
            for (letter, count, what) in &self.gaps {
                let what: String = what.chars().take(140).collect();
                let _ = writeln!(t, "| {letter} | {count} | {} |", what.replace('|', "/"));
            }
        }

        let _ = writeln!(t, "\n## Cache key\n");
        let _ = writeln!(t, "`{}`, from:\n", self.cache_key);
        let _ = writeln!(t, "| input | value |\n|---|---|");
        for (name, value) in &self.key_parts {
            let _ = writeln!(t, "| {name} | `{value}` |");
        }
        if let Some(h) = &self.source_hash {
            let _ = writeln!(t, "\nThe MIR rows the build read hash to `{h}`.");
        }
        t
    }
}

fn section_title(section: Section) -> &'static str {
    match section {
        Section::Dialogue => "Dialogue",
        Section::Scripts => "Scripts",
        Section::Quests => "Quests",
        Section::WorldState => "World state",
        Section::Terminals => "Terminals, notes and locks",
        Section::Packages => "Packages",
        Section::Build => "Build",
    }
}

/// Writes `<out>/sim-report.md`.
pub fn write_report(out: &Path, report: &SimReport) -> Result<(), SimError> {
    std::fs::write(out.join(REPORT_FILE), report.to_markdown()).map_err(|e| fail(REPORT_FILE, e))
}

// ---- The stage ------------------------------------------------------------------

/// What [`sim`] did.
#[derive(Debug, Clone)]
pub struct SimOutcome {
    /// The stored key matched: nothing ran, nothing changed.
    pub cached: bool,
    /// The cache key.
    pub cache_key: String,
    /// The report (`None` on a cache hit).
    pub report: Option<SimReport>,
}

/// The stage: load, check the cache, build, lower, write the layer, the
/// state and the report.
///
/// # Errors
///
/// Bad inputs, a missing `--f2` when the stage writes, or a write that
/// fails. A failed build is a build error of the report, not an error.
pub fn sim(out: &Path, options: &SimOptions) -> Result<SimOutcome, SimError> {
    let inputs = {
        let _t = crate::timings::span("load the run (MIR, layer, index, facts)");
        SimInputs::load(out, options)?
    };
    let key = inputs.cache_key();
    if !options.report_only && !options.force {
        let stored = std::fs::read_to_string(out.join(KEY_FILE)).ok();
        if inputs.has_output
            && stored.as_deref().map(str::trim) == Some(key.as_str())
            && out.join(REPORT_FILE).is_file()
        {
            return Ok(SimOutcome {
                cached: true,
                cache_key: key,
                report: None,
            });
        }
    }
    if !options.report_only && options.f2.is_none() {
        return Err(fail(
            "--f2",
            "the stage writes the layer's base copies (scripts.lst, vault13.gam, ...) from the \
             Fallout 2 install: give --f2 <dir>, or --report-only to build in memory",
        ));
    }
    let built = build(&inputs);
    let mut maps = Vec::new();
    if !options.report_only
        && let Some(lowered) = &built.lowered
    {
        let (attached, state) = write_layer(&inputs, lowered)?;
        maps = attached;
        if !inputs.has_output {
            inputs.pristine.write(out)?;
        }
        let dir = out.join(STATE_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| fail(STATE_DIR, e))?;
        let text = serde_json::to_string_pretty(&state).unwrap_or_default() + "\n";
        std::fs::write(state_path(out), text).map_err(|e| fail(STATE_DIR, e))?;
    }
    let report = SimReport::new(&inputs, &built, maps, options.report_only);
    write_report(out, &report)?;
    if !options.report_only {
        // A build error writes no key: the next run tries again.
        if report.errors.is_empty() {
            std::fs::write(out.join(KEY_FILE), format!("{key}\n"))
                .map_err(|e| fail(KEY_FILE, e))?;
        } else {
            let _ = std::fs::remove_file(out.join(KEY_FILE));
        }
    }
    Ok(SimOutcome {
        cached: false,
        cache_key: key,
        report: Some(report),
    })
}

/// The command: run, print.
pub fn cli(out: &Path, options: &SimOptions) -> Result<(), String> {
    let started = std::time::Instant::now();
    let outcome = sim(out, options).map_err(|e| e.to_string())?;
    match &outcome.report {
        None => println!("sim: cache hit, nothing to do ({})", outcome.cache_key),
        Some(report) => {
            for warning in &report.warnings {
                eprintln!("warning: {warning}");
            }
            for line in report.console_lines() {
                println!("{line}");
            }
            println!("report: {}", out.join(REPORT_FILE).display());
            // The report is written; a build error is still a failed run.
            if let Some(failure) = report.failure() {
                for error in &report.errors {
                    eprintln!("build error: {error}");
                }
                println!("time: {:.2} s", started.elapsed().as_secs_f64());
                return Err(failure);
            }
        }
    }
    println!("time: {:.2} s", started.elapsed().as_secs_f64());
    Ok(())
}
