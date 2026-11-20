//! The extractor: from an install's data directory and a plugin list to a
//! MIR directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::actors;
use crate::assets::{self, AssetCounts, write_atomic};
use crate::cells;
use crate::db;
use crate::error::MirError;
use crate::index::{Index, load_plugins};
use crate::model::{
    ActorEntry, ArchiveEntry, AssetStatus, CellDoc, CellKey, Manifest, PartRole, PluginEntry,
    RawStore, SCHEMA_VERSION, Slice,
};
use crate::paths::form_file_stem;
use crate::reader::{
    ACTORS, ANIMATIONS, CELLS, INTERIORS, MANIFEST, MESHES, Mir, RECORDS, REPORT, SKELETONS,
    TEXTURES, actor_path, cell_path, interior_path, read_json,
};
use crate::report::{ReportParams, report};
use crate::source::DataSource;

/// What to extract and where to.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractOptions {
    /// The install's `Data` directory.
    pub data_dir: PathBuf,
    /// Plugin file names, in load order; the last one wins a form.
    pub plugins: Vec<String>,
    /// The MIR directory to write. It is created when missing; a previous
    /// extract in it is reused where its inputs are unchanged.
    pub out: PathBuf,
    /// The part of a worldspace to extract.
    pub slice: Slice,
    /// Threads for decoding records and converting assets; 0 counts as 1.
    /// Output bytes do not depend on it.
    pub jobs: usize,
    /// How much of the raw store to keep.
    pub raw_store: RawStore,
}

/// What an extract did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractSummary {
    /// Whether `records.sqlite` was built by this run (`false`: an earlier
    /// run's file was kept because its inputs are unchanged).
    pub records_rebuilt: bool,
    /// Records in `records.sqlite`; 0 when it was not rebuilt.
    pub records: usize,
    /// Raw subrecords stored; 0 when it was not rebuilt.
    pub raw_subrecords: usize,
    /// Undecodable records listed in `records.sqlite`; 0 when it was not
    /// rebuilt.
    pub bad_records: usize,
    /// Exterior cells written.
    pub exterior_cells: usize,
    /// Interior cells written.
    pub interior_cells: usize,
    /// References in the written cells.
    pub references: usize,
    /// Actor documents written.
    pub actors: usize,
    /// Mesh conversion counts.
    pub meshes: AssetCounts,
    /// Texture conversion counts.
    pub textures: AssetCounts,
    /// Skeleton conversion counts: the actor documents' skeleton paths.
    pub skeletons: AssetCounts,
    /// Animation conversion counts: the actor documents' stand and walk
    /// paths.
    pub animations: AssetCounts,
    /// Problems listed in the manifest.
    pub issues: usize,
    /// Wall-clock time of each step, in order. Not written anywhere.
    pub timings: Vec<(String, Duration)>,
}

fn create_dir(path: &Path) -> Result<(), MirError> {
    fs::create_dir_all(path).map_err(|e| MirError::io(path, e))
}

fn json_bytes<T: serde::Serialize>(value: &T, pretty: bool) -> Result<Vec<u8>, MirError> {
    let mut bytes = if pretty {
        serde_json::to_vec_pretty(value)
    } else {
        serde_json::to_vec(value)
    }
    .map_err(|source| MirError::Json {
        path: String::new(),
        source,
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Deletes `*.json` files under `dir` that are not in `keep` (relative
/// paths), and empty folders left behind.
fn remove_stale(root: &Path, dir: &str, keep: &BTreeSet<PathBuf>) -> Result<(), MirError> {
    fn walk(
        root: &Path,
        relative: &Path,
        keep: &BTreeSet<PathBuf>,
        depth: usize,
    ) -> Result<(), MirError> {
        if depth > 4 {
            return Ok(());
        }
        let path = root.join(relative);
        let Ok(listing) = fs::read_dir(&path) else {
            return Ok(());
        };
        let mut names: Vec<(PathBuf, bool)> = listing
            .filter_map(Result::ok)
            .map(|e| {
                (
                    relative.join(e.file_name()),
                    e.file_type().is_ok_and(|t| t.is_dir()),
                )
            })
            .collect();
        names.sort();
        for (name, is_dir) in names {
            if is_dir {
                walk(root, &name, keep, depth + 1)?;
                let _ = fs::remove_dir(root.join(&name));
            } else if name.extension().is_some_and(|e| e == "json") && !keep.contains(&name) {
                let full = root.join(&name);
                fs::remove_file(&full).map_err(|e| MirError::io(&full, e))?;
            }
        }
        Ok(())
    }
    walk(root, Path::new(dir), keep, 0)
}

/// The name of a part role, as its JSON form writes it.
fn role_name(role: PartRole) -> &'static str {
    match role {
        PartRole::Body => "body",
        PartRole::Outfit => "outfit",
        PartRole::Head => "head",
        PartRole::Hair => "hair",
    }
}

/// Extracts a slice of an install into a MIR directory.
///
/// `records.sqlite` covers every record of the plugin list; cells,
/// interiors, meshes and textures cover the slice; `skeletons/` and
/// `animations/` cover the actor documents' skeleton, stand and walk
/// paths. A bad record, a missing mesh, an undecodable texture or a
/// skeleton or animation that does not convert is reported, never fatal.
///
/// # Errors
///
/// When a plugin cannot be read or its header does not parse, the slice's
/// worldspace does not exist, or an output cannot be written.
pub fn extract(options: &ExtractOptions) -> Result<ExtractSummary, MirError> {
    if options.plugins.is_empty() {
        return Err(MirError::Options("the plugin list is empty".to_owned()));
    }
    if options.slice.x.is_empty() || options.slice.y.is_empty() {
        return Err(MirError::Options(
            "the slice's cell range is empty".to_owned(),
        ));
    }
    let jobs = options.jobs.max(1);
    let out = &options.out;
    let mut summary = ExtractSummary::default();
    let mut clock = Instant::now();
    let mut lap = |summary: &mut ExtractSummary, step: &str| {
        summary.timings.push((step.to_owned(), clock.elapsed()));
        clock = Instant::now();
    };

    for dir in [
        out.clone(),
        out.join(MESHES),
        out.join(TEXTURES),
        out.join(CELLS),
        out.join(INTERIORS),
        out.join(ACTORS),
        out.join(SKELETONS),
        out.join(ANIMATIONS),
    ] {
        create_dir(&dir)?;
    }
    let previous: Option<Manifest> = read_json(out, Path::new(MANIFEST)).ok();

    let files = load_plugins(&options.data_dir, &options.plugins)?;
    let plugins: Vec<PluginEntry> = files
        .iter()
        .map(|f| PluginEntry {
            name: f.name.clone(),
            size: f.bytes.len() as u64,
            sha256: f.sha256.clone(),
        })
        .collect();
    let source = DataSource::new(&options.data_dir, &options.plugins)?;
    lap(&mut summary, "read plugins");
    let index = Index::build(&files)?;
    lap(&mut summary, "index records");

    let records_path = out.join(RECORDS);
    let reusable = previous.as_ref().is_some_and(|m| {
        m.schema_version == SCHEMA_VERSION
            && m.plugins == plugins
            && m.raw_store == options.raw_store
    }) && records_path.is_file();
    if !reusable {
        let stats = db::build(&records_path, &index, &files, options.raw_store, jobs)?;
        summary.records_rebuilt = true;
        summary.records = stats.records;
        summary.raw_subrecords = stats.raw_subrecords;
        summary.bad_records = stats.bad_records;
    }
    lap(&mut summary, "records.sqlite");

    let built = cells::build(&index, &options.slice)?;
    lap(&mut summary, "cells");
    let mut exteriors = built.exteriors;
    let mut interiors = built.interiors;

    let mut built_actors = actors::build(
        &index,
        &source,
        exteriors
            .iter()
            .chain(interiors.iter().map(|(_, d)| d))
            .flat_map(|d| &d.references),
    );
    lap(&mut summary, "actors");

    // The references' models and the actors' parts.
    let mut models: BTreeSet<String> = std::mem::take(&mut built_actors.meshes);
    for doc in exteriors.iter().chain(interiors.iter().map(|(_, d)| d)) {
        models.extend(doc.references.iter().filter_map(|r| r.model.clone()));
    }
    let models: Vec<String> = models.into_iter().collect();
    let meshes = assets::meshes(&source, &models, &out.join(MESHES), jobs);
    summary.meshes = meshes.counts;
    lap(&mut summary, "meshes");

    let mut texture_paths: BTreeSet<String> = BTreeSet::new();
    for entry in meshes.entries.values() {
        if let Some(info) = entry.converted_hash().and_then(|h| meshes.info.get(h)) {
            texture_paths.extend(info.textures.iter().cloned());
        }
    }
    for doc in &exteriors {
        if let Some(land) = &doc.land {
            texture_paths.extend(land.layers.iter().filter_map(|l| l.texture.clone()));
        }
    }
    texture_paths.extend(std::mem::take(&mut built_actors.textures));
    let texture_paths: Vec<String> = texture_paths.into_iter().collect();
    let textures = assets::textures(&source, &texture_paths, &out.join(TEXTURES), jobs);
    summary.textures = textures.counts;
    lap(&mut summary, "textures");

    // The actors' skeletons and animations.
    let mut skeleton_paths: BTreeSet<String> = BTreeSet::new();
    let mut animation_paths: BTreeSet<String> = BTreeSet::new();
    for doc in &built_actors.docs {
        skeleton_paths.extend(doc.skeleton_path.clone());
        animation_paths.extend(doc.stand_path.clone());
        animation_paths.extend(doc.walk_path.clone());
    }
    let skeleton_paths: Vec<String> = skeleton_paths.into_iter().collect();
    let animation_paths: Vec<String> = animation_paths.into_iter().collect();
    let skeletons = assets::skeletons(&source, &skeleton_paths, &out.join(SKELETONS), jobs);
    summary.skeletons = skeletons.counts;
    let animations = assets::animations(&source, &animation_paths, &out.join(ANIMATIONS), jobs);
    summary.animations = animations.counts;
    lap(&mut summary, "skeletons and animations");

    // Fill in the hashes and write the documents.
    let fill = |doc: &mut CellDoc| {
        for r in &mut doc.references {
            r.mesh = r
                .model
                .as_ref()
                .and_then(|m| meshes.entries.get(m))
                .and_then(|e| e.converted_hash())
                .map(str::to_owned);
        }
        if let Some(land) = &mut doc.land {
            for layer in &mut land.layers {
                layer.texture_hash = layer
                    .texture
                    .as_ref()
                    .and_then(|t| textures.entries.get(t))
                    .and_then(|e| e.converted_hash())
                    .map(str::to_owned);
            }
        }
    };
    let mut keep_cells = BTreeSet::new();
    let mut keep_interiors = BTreeSet::new();
    let mut cell_keys = Vec::new();
    for doc in &mut exteriors {
        fill(doc);
        let crate::model::CellLocation::Exterior {
            worldspace, x, y, ..
        } = &doc.location
        else {
            continue;
        };
        let relative = cell_path(worldspace, *x, *y);
        if let Some(parent) = out.join(&relative).parent() {
            create_dir(parent)?;
        }
        write_atomic(&out.join(&relative), &json_bytes(doc, false)?)?;
        cell_keys.push(CellKey {
            worldspace: worldspace.clone(),
            x: *x,
            y: *y,
        });
        keep_cells.insert(relative);
        summary.references += doc.references.len();
    }
    for (entry, doc) in &mut interiors {
        fill(doc);
        let relative = interior_path(&entry.id);
        write_atomic(&out.join(&relative), &json_bytes(doc, false)?)?;
        keep_interiors.insert(relative);
        summary.references += doc.references.len();
    }
    let mut keep_actors = BTreeSet::new();
    let mut actor_entries = Vec::new();
    let texture_hash = |path: &str| {
        textures
            .entries
            .get(path)
            .and_then(|e| e.converted_hash())
            .map(str::to_owned)
    };
    let status_text = |status: AssetStatus| match status {
        AssetStatus::Converted => "converted",
        AssetStatus::Missing => "missing",
        AssetStatus::Unreadable => "unreadable",
        AssetStatus::Undecodable => "undecodable",
    };
    // The hash of a path in a store, or a note saying why there is none.
    // A path the resolver already noted as missing is not noted again.
    let link = |doc: &mut crate::model::ActorDoc,
                what: &str,
                path: &str,
                entries: &BTreeMap<String, crate::model::AssetEntry>|
     -> Option<String> {
        let entry = entries.get(path);
        if let Some(hash) = entry.and_then(|e| e.converted_hash()) {
            return Some(hash.to_owned());
        }
        let status = entry.map_or(AssetStatus::Missing, |e| e.status);
        let noted = status == AssetStatus::Missing && doc.notes.iter().any(|n| n.contains(path));
        if !noted {
            doc.notes
                .push(format!("{what} {path} is {}", status_text(status)));
        }
        None
    };
    for doc in &mut built_actors.docs {
        if let Some(path) = doc.skeleton_path.clone() {
            doc.skeleton = link(doc, "skeleton", &path, &skeletons.entries);
        }
        let stand = doc
            .stand_path
            .clone()
            .and_then(|p| link(doc, "standing idle", &p, &animations.entries));
        let walk = doc
            .walk_path
            .clone()
            .and_then(|p| link(doc, "walk", &p, &animations.entries));
        doc.animations = match (stand, walk) {
            (Some(stand), Some(walk)) => Some(crate::model::ActorAnimations { stand, walk }),
            _ => None,
        };
        for part in &mut doc.parts {
            let entry = meshes.entries.get(&part.path);
            part.mesh = entry.and_then(|e| e.converted_hash()).map(str::to_owned);
            if part.mesh.is_none() {
                let status = entry.map_or(AssetStatus::Missing, |e| e.status);
                doc.notes.push(format!(
                    "{} part {} is {}",
                    role_name(part.role),
                    part.path,
                    status_text(status)
                ));
            }
            if let Some(t) = &part.texture
                && texture_hash(t).is_none()
            {
                let status = textures
                    .entries
                    .get(t)
                    .map_or(AssetStatus::Missing, |e| e.status);
                doc.notes
                    .push(format!("texture {t} is {}", status_text(status)));
            }
        }
        doc.notes.dedup();
        doc.appearance = actors::appearance(doc, &texture_hash);
        let id = form_file_stem(&doc.form);
        let relative = actor_path(&id);
        write_atomic(&out.join(&relative), &json_bytes(doc, true)?)?;
        keep_actors.insert(relative);
        actor_entries.push(ActorEntry {
            id,
            form: doc.form.clone(),
            kind: doc.kind,
        });
    }
    actor_entries.sort_by(|a, b| a.form.cmp(&b.form));
    summary.actors = actor_entries.len();
    remove_stale(out, CELLS, &keep_cells)?;
    remove_stale(out, INTERIORS, &keep_interiors)?;
    remove_stale(out, ACTORS, &keep_actors)?;
    cell_keys.sort();
    summary.exterior_cells = cell_keys.len();
    summary.interior_cells = interiors.len();
    summary.issues = built.issues.len();
    lap(&mut summary, "cell documents");

    let errors: BTreeMap<String, String> = source.archive_errors().into_iter().collect();
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        slice: Slice {
            worldspace: built.worldspace.clone(),
            ..options.slice.clone()
        },
        raw_store: options.raw_store,
        plugins,
        archives: source
            .archive_names()
            .into_iter()
            .map(|name| ArchiveEntry {
                error: errors.get(&name).cloned(),
                name,
            })
            .collect(),
        cells: cell_keys,
        interiors: interiors.iter().map(|(e, _)| e.clone()).collect(),
        actors: actor_entries,
        meshes: meshes.entries,
        textures: textures.entries,
        mesh_info: meshes.info,
        texture_info: textures.info,
        skeletons: skeletons.entries,
        animations: animations.entries,
        skeleton_info: skeletons.info,
        animation_info: animations.info,
        issues: built.issues,
    };
    write_atomic(&out.join(MANIFEST), &json_bytes(&manifest, true)?)?;

    let mir = Mir::open(out)?;
    let text = report(&mir, &ReportParams::default())?;
    write_atomic(&out.join(REPORT), text.as_bytes())?;
    lap(&mut summary, "manifest and report");
    Ok(summary)
}
