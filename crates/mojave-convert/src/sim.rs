//! The simulation stage's scaffold: `mojave-convert sim --out <dir>`.
//!
//! Pass 2 (`docs/simulation-layer-spec.md`, section 12, task T9) compiles
//! dialogue, quests, scripts and packages into the layer. This module is
//! the entry point it will grow into, and nothing more yet: it opens a
//! finished output directory (the MIR, the built layer under `mods/mojave`,
//! and `map-placements.json` or the reconstruction of it), checks that they
//! belong together, counts what the source has for the placed references,
//! computes the stage cache key of section 10.3, and writes a stub
//! `sim-report.md` beside `map-report.md`.
//!
//! It never renders, slices or builds a map, never calls the terrain,
//! sprite or critter stages, never opens the game install, and never
//! changes a file of the layer. Everything it reads is under `--out`.
//!
//! Shape for Pass 2: [`SimInputs::load`] reads everything once; [`run`] is
//! a pure function of it; the command line in [`cli`] is thin. The
//! transpiler plugs into `run`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use mir::{FormKey, GameVariant, MirError, RefReason};
use serde::Serialize;
use vault_formats::mod_manifest::{MANIFEST, ModManifest};

use crate::map::LAYER_ID;
use crate::placements::{self, Origin, PlacedKind, PlacementError, PlacementIndex, UnplacedReason};

/// The stage's cache key version (spec 10.3: `sim-1`).
pub const SIM_VERSION: &str = "sim-1";

/// The report's name, beside `map-report.md`.
pub const REPORT_FILE: &str = "sim-report.md";

/// Files of the layer the spec's section 10.1 lists as base copies. They
/// are part of the cache key when the layer has them (a layer from the map
/// stage has none yet).
const BASE_FILES: [&str; 7] = [
    "scripts/scripts.lst",
    "data/vault13.gam",
    "data/quests.txt",
    "data/holodisk.txt",
    "text/english/game/quests.msg",
    "text/english/game/pipboy.msg",
    "text/english/game/map.msg",
];

/// The record signatures the report lists for the whole extract.
const LOGIC_SIGNATURES: [(&str, &str); 7] = [
    ("DIAL", "dialogue topics"),
    ("INFO", "dialogue infos"),
    ("SCPT", "scripts"),
    ("PACK", "packages"),
    ("TERM", "terminals"),
    ("NOTE", "notes"),
    ("QUST", "quests"),
];

/// A failure of the stage's inputs.
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

impl From<MirError> for SimError {
    fn from(error: MirError) -> Self {
        fail("mir", error)
    }
}

/// What the source says about one base record that placed references use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseLogic {
    /// Its attached script.
    pub script: Option<String>,
    /// The script's source text is in the MIR.
    pub script_has_source: bool,
    /// Its AI packages (actors).
    pub packages: Vec<String>,
    /// The dialogue infos it speaks (by speaker or by a `GetIsID` condition).
    pub infos: BTreeSet<String>,
    /// The topics of those infos.
    pub topics: BTreeSet<String>,
    /// The quests of those infos.
    pub quests: BTreeSet<String>,
}

/// What the MIR holds for the placed set, read once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogicInputs {
    /// Per base form key.
    pub bases: BTreeMap<String, BaseLogic>,
    /// Placed terminal bases that have a terminal record.
    pub terminals: BTreeSet<String>,
    /// Placed note bases that have a note record.
    pub notes: BTreeSet<String>,
    /// Packages that name a placed reference as their target or location.
    pub packages_naming_refs: BTreeSet<String>,
    /// Quests with an objective target on a placed reference.
    pub quests_targeting_refs: BTreeSet<String>,
    /// Per reason, how many distinct placed references something points at.
    pub referenced: BTreeMap<RefReason, usize>,
    /// Record counts of the whole extract, by signature.
    pub totals: BTreeMap<String, u64>,
}

/// Everything the stage reads, loaded once.
#[derive(Debug, Clone)]
pub struct SimInputs {
    /// The source game.
    pub game: GameVariant,
    /// The layer's name.
    pub layer: String,
    /// The placement index, from the file or reconstructed. Placed
    /// references that lack a base but name a reference have it filled in
    /// from the MIR.
    pub index: PlacementIndex,
    /// The index came from `map-placements.json`.
    pub from_file: bool,
    /// Lines the run prints and the report keeps.
    pub warnings: Vec<String>,
    /// The source's logic for the placed set.
    pub logic: LogicInputs,
    /// What the cache key is made of, in order: a name and a value each.
    pub key_parts: Vec<(String, String)>,
}

fn soft<T: Default>(
    result: Result<T, MirError>,
    what: &str,
    warnings: &mut Vec<String>,
) -> Result<T, SimError> {
    match result {
        Ok(value) => Ok(value),
        Err(MirError::MissingTable(table)) => {
            warnings.push(format!(
                "the MIR has no `{table}` table (extracted before records revision 1): {what} \
                 left out; run extract again"
            ));
            Ok(T::default())
        }
        Err(other) => Err(other.into()),
    }
}

fn sql(error: impl std::fmt::Display) -> SimError {
    fail("records.sqlite", error)
}

fn digest(parts: &[&str]) -> String {
    let mut text = String::new();
    for part in parts {
        text.push_str(part);
        text.push('\n');
    }
    mir::content_hash(text.as_bytes())
}

fn json_of(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

impl SimInputs {
    /// Opens an output directory and reads everything the stage uses.
    ///
    /// # Errors
    ///
    /// The MIR or the layer is missing or does not open, the index belongs
    /// to another game or names a map the layer lacks, or a query fails.
    pub fn load(out: &Path) -> Result<Self, SimError> {
        let mir_dir = out.join("mir");
        let mir = mir::Mir::open(&mir_dir).map_err(|e| {
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

        let (index, from_file) = match placements::read(out)? {
            Some(index) => (index, true),
            None => {
                warnings.push(format!(
                    "{} not found: reconstructed the placement index from the built maps \
                     (no references, bases or enable parents; the counts below that need them \
                     are empty). Run the map stage with the placement index for the full \
                     picture.",
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
            let registered = manifest
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
        for registration in &manifest.maps {
            let stem = registration.file.trim_end_matches(".map");
            if index.map(stem).is_none() {
                warnings.push(format!("map {stem} is in the layer but not in the index"));
            }
        }

        let mut key_parts: Vec<(String, String)> = vec![
            ("sim".into(), SIM_VERSION.into()),
            ("transpiler".into(), "none (Pass 2 scaffold)".into()),
            (
                "foundry.toml [sim]".into(),
                "none (not read by the scaffold)".into(),
            ),
            ("game".into(), game.id().into()),
            (
                "slice".into(),
                mir::content_hash(json_of(&mir.manifest().slice).as_bytes()),
            ),
            (
                "plugins".into(),
                mir::content_hash(json_of(&mir.manifest().plugins).as_bytes()),
            ),
        ];
        let index_hash = mir::content_hash(index.to_text().as_bytes());

        let (index, logic, rows) = {
            let mut index = index;
            let records = mir.records()?;
            let first_plugin = mir
                .manifest()
                .plugins
                .first()
                .map(|p| p.name.clone())
                .unwrap_or_default();
            let logic = read_logic(&records, &mut index, &first_plugin, &mut warnings)?;
            (index, logic.0, logic.1)
        };
        key_parts.push(("mir rows".into(), rows));

        for path in BASE_FILES {
            let value = match std::fs::read(layer_dir.join(path)) {
                Ok(bytes) => mir::content_hash(&bytes),
                Err(_) => "absent".into(),
            };
            key_parts.push((format!("base file {path}"), value));
        }
        let map_key = std::fs::read_to_string(out.join("cache").join("map.key"))
            .map_or_else(|_| "absent".to_owned(), |t| t.trim().to_owned());
        key_parts.push(("map stage key".into(), map_key));
        let mut maps: Vec<_> = manifest.maps.iter().collect();
        maps.sort_by(|a, b| a.file.cmp(&b.file));
        for registration in maps {
            let path = layer_dir.join("maps").join(&registration.file);
            let bytes =
                std::fs::read(&path).map_err(|e| fail(format!("maps/{}", registration.file), e))?;
            key_parts.push((
                format!("map {} ({})", registration.file, registration.number),
                mir::content_hash(&bytes),
            ));
        }
        key_parts.push(("placement index".into(), index_hash));

        Ok(Self {
            game,
            layer: index.layer.clone(),
            index,
            from_file,
            warnings,
            logic,
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
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        format!("{SIM_VERSION}:{}", digest(&lines))
    }
}

/// The MIR queries; also fills in the bases of placed references that
/// name only a reference. Returns the logic and the hash of every row read.
fn read_logic(
    records: &mir::Records,
    index: &mut PlacementIndex,
    first_plugin: &str,
    warnings: &mut Vec<String>,
) -> Result<(LogicInputs, String), SimError> {
    let mut rows = String::new();
    let mut logic = LogicInputs {
        totals: records.record_counts()?,
        ..LogicInputs::default()
    };

    // A reference known only by its form key (the reconstruction names the
    // people of an interior map) gets its base from the MIR.
    for entry in &mut index.placed {
        if entry.base.is_some() {
            continue;
        }
        let Some(reference) = entry
            .reference
            .as_deref()
            .and_then(|r| r.parse::<FormKey>().ok())
        else {
            continue;
        };
        if let Some(row) = records.placed_ref(&reference)? {
            entry.base = row.base.map(|b| b.to_string());
            entry.base_kind = row.base_sig;
        }
    }

    let placed_refs: BTreeSet<&str> = index
        .placed
        .iter()
        .filter_map(|p| p.reference.as_deref())
        .collect();
    let bases: BTreeMap<&str, Option<&str>> = index
        .placed
        .iter()
        .filter_map(|p| p.base.as_deref().map(|b| (b, p.base_kind.as_deref())))
        .collect();

    // Scripts, packages, terminals, notes of each base.
    let notes = if bases.values().any(|k| *k == Some("NOTE")) {
        soft(records.notes(), "notes", warnings)?
    } else {
        Vec::new()
    };
    for (base, signature) in &bases {
        let Ok(key) = base.parse::<FormKey>() else {
            continue;
        };
        let mut entry = BaseLogic::default();
        if let Some(object) = records.base_object(&key)?
            && let Some(script) = object.script
        {
            if let Some(source) = records.script_source(&script)? {
                entry.script_has_source = true;
                let _ = writeln!(
                    rows,
                    "script {script} {}",
                    mir::content_hash(source.as_bytes())
                );
            }
            entry.script = Some(script.to_string());
        }
        let is_actor = matches!(*signature, Some("NPC_" | "CREA"));
        if is_actor {
            let packages = soft(records.actor_packages(&key), "packages", warnings)?;
            for package in &packages {
                if let Some(row) = soft(records.package(package), "packages", warnings)? {
                    let _ = writeln!(rows, "package {}", json_of(&row));
                }
            }
            entry.packages = packages.iter().map(ToString::to_string).collect();
        }
        if *signature == Some("TERM")
            && let Some(row) = soft(records.terminal(&key), "terminals", warnings)?
        {
            let _ = writeln!(rows, "terminal {}", json_of(&row));
            logic.terminals.insert(base.to_string());
        }
        if *signature == Some("NOTE")
            && let Some(row) = notes.iter().find(|n| n.form == key)
        {
            let _ = writeln!(rows, "note {}", json_of(row));
            logic.notes.insert(base.to_string());
        }
        logic.bases.insert((*base).to_owned(), entry);
    }

    // Dialogue: infos with the base as speaker, and infos whose conditions
    // ask `GetIsID` (function 72) for it. The condition's parameter is a
    // raw id, resolved here for the first plugin only (high byte 0).
    let connection = records.connection();
    let mut infos: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    {
        let mut statement = connection
            .prepare("SELECT form, topic, quest, speaker FROM infos ORDER BY form")
            .map_err(sql)?;
        let found = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(sql)?;
        for row in found {
            let (form, topic, quest, speaker) = row.map_err(sql)?;
            if let Some(base) = speaker.and_then(|s| logic.bases.get_mut(&s)) {
                base.infos.insert(form.clone());
            }
            infos.insert(form, (topic, quest));
        }
    }
    {
        let mut statement = connection
            .prepare(
                "SELECT owner, param1 FROM conditions WHERE function = 72 AND context = 'info' \
                 ORDER BY owner, seq",
            )
            .map_err(sql)?;
        let found = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(sql)?;
        for row in found {
            let (owner, raw) = row.map_err(sql)?;
            if !(0..=0x00FF_FFFF).contains(&raw) {
                continue;
            }
            let base = FormKey::new(first_plugin, raw as u32).to_string();
            if let Some(entry) = logic.bases.get_mut(&base) {
                entry.infos.insert(owner);
            }
        }
    }
    for entry in logic.bases.values_mut() {
        for info in &entry.infos {
            if let Some((topic, quest)) = infos.get(info) {
                entry.topics.insert(topic.clone());
                entry.quests.extend(quest.clone());
                let _ = writeln!(
                    rows,
                    "info {info} {topic} {}",
                    quest.as_deref().unwrap_or("-")
                );
            }
        }
    }

    // What points at the placed references.
    let referrals = soft(records.ref_referrals(), "referrals", warnings)?;
    let mut per_reason: BTreeMap<RefReason, BTreeSet<String>> = BTreeMap::new();
    for referral in referrals {
        let reference = referral.reference.to_string();
        if !placed_refs.contains(reference.as_str()) {
            continue;
        }
        let by = referral.by.to_string();
        match referral.reason {
            RefReason::QuestTarget => {
                logic.quests_targeting_refs.insert(by.clone());
            }
            RefReason::PackageTarget | RefReason::PackageLocation => {
                logic.packages_naming_refs.insert(by.clone());
            }
            _ => {}
        }
        let _ = writeln!(
            rows,
            "referral {reference} {:?} {by} {}",
            referral.reason, referral.context
        );
        per_reason
            .entry(referral.reason)
            .or_default()
            .insert(reference);
    }
    logic.referenced = per_reason.into_iter().map(|(k, v)| (k, v.len())).collect();

    Ok((logic, mir::content_hash(rows.as_bytes())))
}

// ---- The report -------------------------------------------------------------

/// Counts for one map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapCounts {
    /// The file stem.
    pub stem: String,
    /// The map's number.
    pub number: u32,
    /// Placed references by kind.
    pub placed: BTreeMap<PlacedKind, usize>,
    /// References of its cells that are not on it.
    pub unplaced: usize,
}

/// What the source has for the placed set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogicCounts {
    /// Distinct scripts attached to placed bases, and those with source.
    pub scripts: usize,
    /// Of them, with source text in the MIR.
    pub scripts_with_source: usize,
    /// Distinct packages: of placed actors, and naming a placed reference.
    pub packages: usize,
    /// Distinct dialogue infos spoken by placed bases.
    pub infos: usize,
    /// Distinct topics of those infos.
    pub topics: usize,
    /// Distinct quests: of those infos, and with a target on a placed
    /// reference.
    pub quests: usize,
    /// Placed terminals with a terminal record.
    pub terminals: usize,
    /// Placed notes with a note record.
    pub notes: usize,
}

/// How many of a group of placed references have something to compile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Placed references in the group.
    pub placed: usize,
    /// With a base script.
    pub script: usize,
    /// With AI packages.
    pub packages: usize,
    /// With dialogue.
    pub dialogue: usize,
    /// With any of the three.
    pub any: usize,
    /// Whose base is unknown (no reference in the index).
    pub unresolved: usize,
}

/// The stage's result: counts, no programs yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimReport {
    /// The source game's name.
    pub game: String,
    /// The game's short id.
    pub game_id: String,
    /// The layer's name.
    pub layer: String,
    /// Where the placement index came from.
    pub origin: Origin,
    /// The index was read from `map-placements.json`.
    pub from_file: bool,
    /// Warnings of the load.
    pub warnings: Vec<String>,
    /// Per map.
    pub maps: Vec<MapCounts>,
    /// Placed references by kind.
    pub placed: BTreeMap<PlacedKind, usize>,
    /// Unplaced references by reason.
    pub unplaced: BTreeMap<UnplacedReason, usize>,
    /// The source's logic for the placed set.
    pub logic: LogicCounts,
    /// Critters.
    pub critters: Coverage,
    /// Doors, containers, items, terminals and listed scenery.
    pub objects: Coverage,
    /// Distinct placed references something points at, by reason.
    pub referenced: BTreeMap<RefReason, usize>,
    /// Record counts of the whole extract, for the signatures of
    /// [`LOGIC_SIGNATURES`].
    pub totals: BTreeMap<String, u64>,
    /// The stage cache key.
    pub cache_key: String,
    /// The parts of the key.
    pub key_parts: Vec<(String, String)>,
}

/// Counts the inputs. Pass 2 replaces the body with the build.
pub fn run(inputs: &SimInputs) -> SimReport {
    let index = &inputs.index;
    let logic = &inputs.logic;
    let mut maps: Vec<MapCounts> = index
        .maps
        .iter()
        .map(|m| MapCounts {
            stem: m.stem.clone(),
            number: m.number,
            placed: BTreeMap::new(),
            unplaced: index.unplaced.iter().filter(|u| u.map == m.stem).count(),
        })
        .collect();
    let mut placed: BTreeMap<PlacedKind, usize> = BTreeMap::new();
    let mut critters = Coverage::default();
    let mut objects = Coverage::default();
    for entry in &index.placed {
        *placed.entry(entry.kind).or_default() += 1;
        if let Some(map) = maps.iter_mut().find(|m| m.stem == entry.map) {
            *map.placed.entry(entry.kind).or_default() += 1;
        }
        let group = if entry.critter {
            &mut critters
        } else {
            &mut objects
        };
        group.placed += 1;
        match entry.base.as_ref().and_then(|b| logic.bases.get(b)) {
            Some(base) => {
                let (s, p, d) = (
                    base.script.is_some(),
                    !base.packages.is_empty(),
                    !base.infos.is_empty(),
                );
                group.script += usize::from(s);
                group.packages += usize::from(p);
                group.dialogue += usize::from(d);
                group.any += usize::from(s || p || d);
            }
            None => group.unresolved += 1,
        }
    }
    let mut unplaced: BTreeMap<UnplacedReason, usize> = BTreeMap::new();
    for entry in &index.unplaced {
        *unplaced.entry(entry.reason).or_default() += 1;
    }

    let scripts: BTreeSet<&String> = logic
        .bases
        .values()
        .filter_map(|b| b.script.as_ref())
        .collect();
    let scripts_with_source = logic
        .bases
        .values()
        .filter(|b| b.script.is_some() && b.script_has_source)
        .filter_map(|b| b.script.as_ref())
        .collect::<BTreeSet<_>>()
        .len();
    let packages: BTreeSet<&String> = logic
        .bases
        .values()
        .flat_map(|b| &b.packages)
        .chain(&logic.packages_naming_refs)
        .collect();
    let infos: BTreeSet<&String> = logic.bases.values().flat_map(|b| &b.infos).collect();
    let topics: BTreeSet<&String> = logic.bases.values().flat_map(|b| &b.topics).collect();
    let quests: BTreeSet<&String> = logic
        .bases
        .values()
        .flat_map(|b| &b.quests)
        .chain(&logic.quests_targeting_refs)
        .collect();
    let totals = LOGIC_SIGNATURES
        .iter()
        .map(|(sig, _)| {
            (
                (*sig).to_owned(),
                logic.totals.get(*sig).copied().unwrap_or(0),
            )
        })
        .collect();

    SimReport {
        game: inputs.game.display_name().to_owned(),
        game_id: inputs.game.id().to_owned(),
        layer: inputs.layer.clone(),
        origin: index.origin,
        from_file: inputs.from_file,
        warnings: inputs.warnings.clone(),
        maps,
        placed,
        unplaced,
        logic: LogicCounts {
            scripts: scripts.len(),
            scripts_with_source,
            packages: packages.len(),
            infos: infos.len(),
            topics: topics.len(),
            quests: quests.len(),
            terminals: logic.terminals.len(),
            notes: logic.notes.len(),
        },
        critters,
        objects,
        referenced: logic.referenced.clone(),
        totals,
        cache_key: inputs.cache_key(),
        key_parts: inputs.key_parts.clone(),
    }
}

fn kinds_line(counts: &BTreeMap<PlacedKind, usize>) -> String {
    let parts: Vec<String> = PlacedKind::ALL
        .iter()
        .filter_map(|k| counts.get(k).map(|n| format!("{} {}", n, k.label())))
        .collect();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

fn coverage_line(label: &str, c: &Coverage) -> String {
    format!(
        "{label}: {} placed; {} with a script, {} with packages, {} with dialogue ({} with any); \
         {} with no known base",
        c.placed, c.script, c.packages, c.dialogue, c.any, c.unresolved
    )
}

impl SimReport {
    /// The lines the command prints.
    pub fn console_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("game: {} ({})", self.game, self.game_id),
            format!("layer: {}", self.layer),
            format!(
                "placement index: {}",
                if self.from_file {
                    "map-placements.json"
                } else {
                    "reconstructed from the built maps"
                }
            ),
        ];
        for map in &self.maps {
            lines.push(format!(
                "map {} (number {}): {}; {} not placed",
                map.stem,
                map.number,
                kinds_line(&map.placed),
                map.unplaced
            ));
        }
        lines.push(format!("placed: {}", kinds_line(&self.placed)));
        let unplaced: Vec<String> = UnplacedReason::ALL
            .iter()
            .filter_map(|r| self.unplaced.get(r).map(|n| format!("{n} {}", r.label())))
            .collect();
        lines.push(format!(
            "not placed: {}",
            if unplaced.is_empty() {
                "none".to_owned()
            } else {
                unplaced.join(", ")
            }
        ));
        let l = &self.logic;
        lines.push(format!(
            "logic for the placed set: {} scripts ({} with source), {} packages, {} dialogue \
             infos in {} topics, {} quests, {} terminals, {} notes",
            l.scripts,
            l.scripts_with_source,
            l.packages,
            l.infos,
            l.topics,
            l.quests,
            l.terminals,
            l.notes
        ));
        lines.push(coverage_line("critters", &self.critters));
        lines.push(coverage_line("objects", &self.objects));
        lines.push(format!("cache key: {}", self.cache_key));
        lines
    }

    /// The text of `sim-report.md`: no time, host or path, so a second
    /// run on the same output is byte-identical.
    pub fn to_markdown(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "# Simulation layer report (scaffold)\n");
        let _ = writeln!(
            text,
            "No scripts, dialogue or packages are compiled yet: this is the Pass 2 scaffold \
             (`docs/simulation-layer-spec.md` section 12, task T9). The counts below are what the \
             build will have to cover; the layer is untouched.\n"
        );
        let _ = writeln!(text, "## Summary\n");
        let _ = writeln!(text, "- Game: {} (`{}`)", self.game, self.game_id);
        let _ = writeln!(text, "- Layer: {}", self.layer);
        let _ = writeln!(text, "- Maps: {}", self.maps.len());
        let _ = writeln!(
            text,
            "- Placement index: {}",
            if self.from_file {
                "`map-placements.json`"
            } else {
                "reconstructed from the built maps"
            }
        );
        let _ = writeln!(text, "- Placed references: {}", kinds_line(&self.placed));
        let _ = writeln!(text, "- Stage cache key: `{}`", self.cache_key);
        let _ = writeln!(text, "- Programs compiled: 0 (Pass 2)");

        if !self.warnings.is_empty() {
            let _ = writeln!(text, "\n## Warnings\n");
            for warning in &self.warnings {
                let _ = writeln!(text, "- {warning}");
            }
        }

        let _ = writeln!(text, "\n## Placed references\n");
        let _ = writeln!(
            text,
            "| map | number | critters | doors | containers | items | terminals | scenery | not placed |\n\
             |---|---|---|---|---|---|---|---|---|"
        );
        for map in &self.maps {
            let n = |k: PlacedKind| map.placed.get(&k).copied().unwrap_or(0);
            let _ = writeln!(
                text,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                map.stem,
                map.number,
                n(PlacedKind::Critter),
                n(PlacedKind::Door),
                n(PlacedKind::Container),
                n(PlacedKind::Item),
                n(PlacedKind::Terminal),
                n(PlacedKind::Scenery),
                map.unplaced
            );
        }
        let _ = writeln!(text, "\n| not placed, by reason | count |\n|---|---|");
        for reason in UnplacedReason::ALL {
            let _ = writeln!(
                text,
                "| {} | {} |",
                reason.label(),
                self.unplaced.get(&reason).copied().unwrap_or(0)
            );
        }

        let l = &self.logic;
        let _ = writeln!(text, "\n## Source logic for the placed set\n");
        let _ = writeln!(text, "| record | count |\n|---|---|");
        let _ = writeln!(text, "| scripts on placed bases | {} |", l.scripts);
        let _ = writeln!(
            text,
            "| of them with source text | {} |",
            l.scripts_with_source
        );
        let _ = writeln!(
            text,
            "| packages (placed actors, and naming a placed reference) | {} |",
            l.packages
        );
        let _ = writeln!(
            text,
            "| dialogue infos spoken by placed bases | {} |",
            l.infos
        );
        let _ = writeln!(text, "| dialogue topics of those infos | {} |", l.topics);
        let _ = writeln!(
            text,
            "| quests (those infos, and targets on placed references) | {} |",
            l.quests
        );
        let _ = writeln!(text, "| terminals | {} |", l.terminals);
        let _ = writeln!(text, "| notes | {} |", l.notes);

        let _ = writeln!(text, "\n## Coverage\n");
        let _ = writeln!(
            text,
            "| group | placed | script | packages | dialogue | any | no known base |\n|---|---|---|---|---|---|---|"
        );
        for (label, c) in [("critters", &self.critters), ("objects", &self.objects)] {
            let _ = writeln!(
                text,
                "| {label} | {} | {} | {} | {} | {} | {} |",
                c.placed, c.script, c.packages, c.dialogue, c.any, c.unresolved
            );
        }
        if !self.referenced.is_empty() {
            let _ = writeln!(
                text,
                "\n| placed references pointed at, by | count |\n|---|---|"
            );
            for (reason, count) in &self.referenced {
                let _ = writeln!(text, "| {reason:?} | {count} |");
            }
        }

        let _ = writeln!(text, "\n## Whole extract (for scale)\n");
        let _ = writeln!(text, "| record | signature | count |\n|---|---|---|");
        for (sig, name) in LOGIC_SIGNATURES {
            let _ = writeln!(
                text,
                "| {name} | {sig} | {} |",
                self.totals.get(sig).copied().unwrap_or(0)
            );
        }

        let _ = writeln!(text, "\n## Cache key\n");
        let _ = writeln!(text, "`{}`, from:\n", self.cache_key);
        let _ = writeln!(text, "| input | value |\n|---|---|");
        for (name, value) in &self.key_parts {
            let _ = writeln!(text, "| {name} | `{value}` |");
        }
        text
    }
}

/// Writes `<out>/sim-report.md`.
pub fn write_report(out: &Path, report: &SimReport) -> Result<(), SimError> {
    std::fs::write(out.join(REPORT_FILE), report.to_markdown()).map_err(|e| fail(REPORT_FILE, e))
}

/// The command: load, run, write, print.
pub fn cli(out: &Path) -> Result<(), String> {
    let started = std::time::Instant::now();
    let inputs = {
        let _t = crate::timings::span("load the run (MIR, layer, placement index)");
        SimInputs::load(out).map_err(|e| e.to_string())?
    };
    for warning in &inputs.warnings {
        eprintln!("warning: {warning}");
    }
    let report = {
        let _t = crate::timings::span("count the logic");
        run(&inputs)
    };
    {
        let _t = crate::timings::span("write sim-report.md");
        write_report(out, &report).map_err(|e| e.to_string())?;
    }
    for line in report.console_lines() {
        println!("{line}");
    }
    println!("report: {}", out.join(REPORT_FILE).display());
    println!("time: {:.2} s", started.elapsed().as_secs_f64());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placements::{MapEntry, PlacedRef};

    fn placed(map: &str, kind: PlacedKind, base: Option<&str>, object_id: u32) -> PlacedRef {
        PlacedRef {
            map: map.to_owned(),
            map_number: 7,
            elevation: 0,
            hex: object_id as i32,
            object_id: Some(object_id),
            pid: Some(0x0100_0000),
            kind,
            critter: kind == PlacedKind::Critter,
            pieces: 1,
            reference: base.map(|b| format!("r:{b}")),
            editor_id: None,
            base: base.map(str::to_owned),
            base_kind: None,
            base_editor_id: None,
            enable_parent: None,
            initially_disabled: false,
            hidden: false,
            team: None,
            ai_packet: None,
            ..Default::default()
        }
    }

    fn inputs() -> SimInputs {
        let index = PlacementIndex {
            schema_version: placements::SCHEMA_VERSION,
            origin: Origin::Stage,
            layer: "Synthetic".to_owned(),
            game: "fallout3".to_owned(),
            maps: vec![MapEntry {
                stem: "town".to_owned(),
                file: "maps/town.map".to_owned(),
                number: 7,
            }],
            placed: vec![
                placed("town", PlacedKind::Critter, Some("b:talker"), 1),
                placed("town", PlacedKind::Critter, Some("b:walker"), 2),
                placed("town", PlacedKind::Critter, Some("b:plain"), 3),
                placed("town", PlacedKind::Critter, None, 4),
                placed("town", PlacedKind::Door, Some("b:door"), 5),
                placed("town", PlacedKind::Terminal, Some("b:term"), 6),
            ],
            unplaced: Vec::new(),
            notes: Vec::new(),
            ..Default::default()
        };
        let mut logic = LogicInputs::default();
        logic.bases.insert(
            "b:talker".to_owned(),
            BaseLogic {
                script: Some("s:1".to_owned()),
                script_has_source: true,
                infos: ["i:1".to_owned(), "i:2".to_owned()].into(),
                topics: ["t:1".to_owned()].into(),
                quests: ["q:1".to_owned()].into(),
                ..BaseLogic::default()
            },
        );
        logic.bases.insert(
            "b:walker".to_owned(),
            BaseLogic {
                packages: vec!["p:1".to_owned(), "p:2".to_owned()],
                ..BaseLogic::default()
            },
        );
        logic
            .bases
            .insert("b:plain".to_owned(), BaseLogic::default());
        logic.bases.insert(
            "b:door".to_owned(),
            BaseLogic {
                script: Some("s:1".to_owned()),
                ..BaseLogic::default()
            },
        );
        logic.terminals.insert("b:term".to_owned());
        logic.packages_naming_refs.insert("p:2".to_owned());
        logic.packages_naming_refs.insert("p:3".to_owned());
        logic.quests_targeting_refs.insert("q:2".to_owned());
        SimInputs {
            game: GameVariant::Fallout3,
            layer: "Synthetic".to_owned(),
            index,
            from_file: true,
            warnings: Vec::new(),
            logic,
            key_parts: vec![("sim".to_owned(), SIM_VERSION.to_owned())],
        }
    }

    #[test]
    fn the_counts_follow_the_logic_of_the_placed_bases() {
        let report = run(&inputs());
        assert_eq!(report.placed[&PlacedKind::Critter], 4);
        let c = report.critters;
        assert_eq!(
            (
                c.placed,
                c.script,
                c.packages,
                c.dialogue,
                c.any,
                c.unresolved
            ),
            (4, 1, 1, 1, 2, 1)
        );
        let o = report.objects;
        assert_eq!((o.placed, o.script, o.any, o.unresolved), (2, 1, 1, 1));
        let l = report.logic;
        // One script shared by a critter and a door counts once; a package
        // that is both a walker's and the location of a placed reference
        // counts once.
        assert_eq!(
            (
                l.scripts,
                l.scripts_with_source,
                l.packages,
                l.infos,
                l.topics,
                l.quests
            ),
            (1, 1, 3, 2, 1, 2)
        );
        assert_eq!((l.terminals, l.notes), (1, 0));
        assert_eq!(report.maps[0].placed[&PlacedKind::Door], 1);
    }

    #[test]
    fn the_report_is_deterministic_and_the_key_follows_its_parts() {
        let a = inputs();
        let first = run(&a);
        assert_eq!(first.to_markdown(), run(&a).to_markdown());
        assert!(first.to_markdown().contains(&first.cache_key));
        let mut b = inputs();
        b.key_parts
            .push(("map stage key".to_owned(), "x".to_owned()));
        assert_ne!(run(&b).cache_key, first.cache_key);
        assert_eq!(first.cache_key, a.cache_key());
        let lines = first.console_lines().join("\n");
        assert!(
            lines.contains("critters: 4 placed; 1 with a script"),
            "{lines}"
        );
    }
}
