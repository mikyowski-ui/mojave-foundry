//! `report.md`: what an extract found, in deterministic Markdown.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::error::MirError;
use crate::model::{
    ActorDoc, ActorKind, AssetEntry, AssetStatus, CELL_UNITS, CellDoc, PartRole, RefKind, Reference,
};
use crate::reader::{Mir, Records};

/// Settings of the report's estimates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReportParams {
    /// Source units per Fallout 2 floor square (`docs/contracts.md`,
    /// section 4).
    pub tile_units: f32,
    /// Rows shown in each ranking.
    pub top: usize,
}

impl Default for ReportParams {
    fn default() -> Self {
        Self {
            tile_units: 140.0,
            top: 25,
        }
    }
}

/// Fallout 2 holds at most this many entries in one art list.
pub const ART_LIST_LIMIT: u64 = 4096;

/// Floor squares along one side of a Fallout 2 map.
pub const MAP_SQUARES: f32 = 100.0;

/// Base kinds whose references would become walls or scenery.
pub const SCENERY_KINDS: &[&str] = &[
    "ACTI", "DOOR", "FURN", "IDLM", "LIGH", "MSTT", "SCOL", "STAT", "TACT", "TREE",
];

/// Words of the scripting language that are not commands.
const SCRIPT_KEYWORDS: &[&str] = &[
    "array_var",
    "begin",
    "break",
    "continue",
    "else",
    "elseif",
    "end",
    "endif",
    "float",
    "foreach",
    "if",
    "int",
    "let",
    "long",
    "loop",
    "player",
    "ref",
    "reference",
    "return",
    "scn",
    "scriptname",
    "set",
    "short",
    "string_var",
    "to",
    "while",
];

/// Words that declare a variable.
const DECLARATIONS: &[&str] = &[
    "array_var",
    "float",
    "int",
    "long",
    "ref",
    "reference",
    "short",
    "string_var",
];

fn table(out: &mut String, header: &[&str], rows: &[Vec<String>]) {
    let _ = writeln!(out, "| {} |", header.join(" | "));
    let rule: Vec<&str> = header
        .iter()
        .enumerate()
        .map(|(i, _)| if i == 0 { "---" } else { "---:" })
        .collect();
    let _ = writeln!(out, "| {} |", rule.join(" | "));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| c.replace('|', "\\|")).collect();
        let _ = writeln!(out, "| {} |", cells.join(" | "));
    }
    out.push('\n');
}

/// Identifier tokens of one script's source: `(token, follows a dot)`.
/// Comments, strings and numbers are dropped.
fn tokens(source: &str) -> Vec<Vec<(String, bool)>> {
    let mut lines = Vec::new();
    for line in source.lines() {
        let mut out = Vec::new();
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        let mut after_dot = false;
        while i < chars.len() {
            let c = chars[i];
            if c == ';' {
                break;
            } else if c == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                i += 1;
                after_dot = false;
            } else if c.is_ascii_alphabetic() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                out.push((word.to_ascii_lowercase(), after_dot));
                after_dot = false;
            } else if c.is_ascii_digit() {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                after_dot = false;
            } else {
                after_dot = c == '.';
                i += 1;
            }
        }
        lines.push(out);
    }
    lines
}

/// Command identifiers of one script: every identifier that is not a
/// keyword, a declared variable, a block type, the script's own name or
/// the editor id of a record. After a dot, quest variables declared by any
/// script are dropped as well.
fn commands(
    source: &str,
    locals: &BTreeSet<String>,
    all_variables: &BTreeSet<String>,
    editor_ids: &BTreeSet<String>,
) -> Vec<String> {
    let lines = tokens(source);
    let mut declared: BTreeSet<String> = locals.clone();
    for line in &lines {
        if let [(first, false), (name, false), ..] = line.as_slice()
            && DECLARATIONS.contains(&first.as_str())
        {
            declared.insert(name.clone());
        }
    }
    let mut out = Vec::new();
    for line in &lines {
        let mut skip_next = false;
        for (word, after_dot) in line {
            if std::mem::take(&mut skip_next) {
                continue;
            }
            if matches!(word.as_str(), "begin" | "scn" | "scriptname") {
                skip_next = true;
                continue;
            }
            if DECLARATIONS.contains(&word.as_str()) {
                skip_next = true;
                continue;
            }
            if SCRIPT_KEYWORDS.contains(&word.as_str())
                || declared.contains(word)
                || editor_ids.contains(word)
                || (*after_dot && all_variables.contains(word))
            {
                continue;
            }
            out.push(word.clone());
        }
    }
    out
}

/// The footprint estimate of one distinct placement.
struct Footprint {
    model: String,
    yaw: i32,
    scale: i32,
    hexes: u64,
    instances: u64,
}

/// Expected hexes covered by a `w × h` rectangle (floor squares), on a
/// grid of cells a quarter of a floor square in area.
fn hexes_covered(w: f64, h: f64) -> u64 {
    let expected = (2.0 * w + 1.0) * (2.0 * h + 1.0);
    expected.round().max(1.0) as u64
}

struct Budget {
    distinct: usize,
    instances: u64,
    scenery: u64,
    walls: u64,
    /// Wall-strip entries of meshes flatter than `flat_units`.
    flat_walls: u64,
    flat_units: f64,
    unmeasured: u64,
    rows: Vec<Footprint>,
}

fn budget<'d>(
    mir: &Mir,
    docs: impl Iterator<Item = &'d CellDoc>,
    tile_units: f32,
    window: Option<([f32; 2], f32)>,
    yaw_step: f64,
) -> Budget {
    let manifest = mir.manifest();
    let mut groups: BTreeMap<(String, i32, i32), (String, u64)> = BTreeMap::new();
    let mut unmeasured = 0u64;
    for doc in docs {
        for r in &doc.references {
            if r.flags.deleted
                || r.trigger_volume
                || !r
                    .base_kind
                    .as_deref()
                    .is_some_and(|k| SCENERY_KINDS.contains(&k))
            {
                continue;
            }
            let p = r.transform.position;
            if let Some((centre, half)) = window
                && ((p[0] - centre[0]).abs() > half || (p[1] - centre[1]).abs() > half)
            {
                continue;
            }
            let Some(hash) = r.mesh.as_deref() else {
                if r.model.is_some() {
                    unmeasured += 1;
                }
                continue;
            };
            if !manifest
                .mesh_info
                .get(hash)
                .is_some_and(|i| i.bounds.is_some())
            {
                unmeasured += 1;
                continue;
            }
            let degrees = f64::from(r.transform.rotation[2]).to_degrees();
            let yaw =
                (((degrees / yaw_step).round() * yaw_step).round() as i64).rem_euclid(360) as i32;
            let scale = (f64::from(r.transform.scale) * 100.0).round() as i32;
            let model = r.model.clone().unwrap_or_default();
            let entry = groups
                .entry((hash.to_owned(), yaw, scale))
                .or_insert((model, 0));
            entry.1 += 1;
        }
    }
    let mut rows = Vec::new();
    let (mut scenery, mut walls, mut flat_walls, mut instances) = (0u64, 0u64, 0u64, 0u64);
    // Flatter than a quarter of a floor square: roads, decals, rugs.
    let flat_units = f64::from(tile_units) / 4.0;
    for ((hash, yaw, scale), (model, count)) in &groups {
        let Some(bounds) = manifest.mesh_info.get(hash).and_then(|i| i.bounds) else {
            continue;
        };
        let s = f64::from(*scale) / 100.0 / f64::from(tile_units);
        let w = f64::from(bounds[1][0] - bounds[0][0]).max(0.0) * s;
        let h = f64::from(bounds[1][1] - bounds[0][1]).max(0.0) * s;
        let hexes = hexes_covered(w, h);
        let tall = f64::from(bounds[1][2] - bounds[0][2]).max(0.0) * f64::from(*scale) / 100.0;
        if hexes <= 1 {
            scenery += 1;
        } else {
            walls += hexes;
            if tall < flat_units {
                flat_walls += hexes;
            }
        }
        instances += count;
        rows.push(Footprint {
            model: model.clone(),
            yaw: *yaw,
            scale: *scale,
            hexes,
            instances: *count,
        });
    }
    rows.sort_by(|a, b| {
        b.hexes
            .cmp(&a.hexes)
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.yaw.cmp(&b.yaw))
            .then_with(|| a.scale.cmp(&b.scale))
    });
    Budget {
        distinct: groups.len(),
        instances,
        scenery,
        walls,
        flat_walls,
        flat_units,
        unmeasured,
        rows,
    }
}

/// The comparison line for yaw snapped to the six hex directions.
fn write_coarse(out: &mut String, b: &Budget) {
    let _ = writeln!(
        out,
        "With yaw snapped to 60° (the six hex directions) instead: {} distinct placements, \
         {} scenery entries ({}), {} wall-strip entries ({}).\n",
        b.distinct,
        b.scenery,
        headroom(b.scenery),
        b.walls,
        headroom(b.walls)
    );
}

fn headroom(used: u64) -> String {
    if used <= ART_LIST_LIMIT {
        format!("{} entries of headroom", ART_LIST_LIMIT - used)
    } else {
        format!("**over by {} entries**", used - ART_LIST_LIMIT)
    }
}

fn write_budget(out: &mut String, b: &Budget, top: usize) {
    let _ = writeln!(
        out,
        "- distinct placements (mesh, yaw, scale): {} from {} references; {} references without a measurable mesh",
        b.distinct, b.instances, b.unmeasured
    );
    let _ = writeln!(
        out,
        "- scenery entries: {} of {} ({})",
        b.scenery,
        ART_LIST_LIMIT,
        headroom(b.scenery)
    );
    let _ = writeln!(
        out,
        "- wall-strip entries: {} of {} ({}); {} of them from flat meshes (under {} units \
         tall: roads, decals), which would more likely become floor art\n",
        b.walls,
        ART_LIST_LIMIT,
        headroom(b.walls),
        b.flat_walls,
        b.flat_units
    );
    let rows: Vec<Vec<String>> = b
        .rows
        .iter()
        .take(top)
        .map(|f| {
            vec![
                format!("`{}`", f.model),
                f.yaw.to_string(),
                format!("{:.2}", f64::from(f.scale) / 100.0),
                f.hexes.to_string(),
                if f.hexes <= 1 { "scenery" } else { "walls" }.to_owned(),
                f.instances.to_string(),
            ]
        })
        .collect();
    if !rows.is_empty() {
        let _ = writeln!(out, "Top contributors:\n");
        table(
            out,
            &["Model", "Yaw (°)", "Scale", "Hexes", "List", "References"],
            &rows,
        );
    }
}

fn rank<K: Ord + Clone>(counts: &BTreeMap<K, u64>) -> Vec<(K, u64)> {
    let mut list: Vec<(K, u64)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list
}

/// The skeletons and animations of the actor documents: counts, one row
/// per converted path, and every path that failed with its reason.
fn write_actor_stores(out: &mut String, mir: &Mir, docs: &[ActorDoc]) {
    let manifest = mir.manifest();
    let status_name = |status: AssetStatus| match status {
        AssetStatus::Converted => "converted",
        AssetStatus::Missing => "missing",
        AssetStatus::Unreadable => "unreadable",
        AssetStatus::Undecodable => "undecodable",
    };
    let converted = |entries: &BTreeMap<String, AssetEntry>| {
        entries
            .values()
            .filter(|e| e.converted_hash().is_some())
            .count()
    };
    let npcs: Vec<&ActorDoc> = docs.iter().filter(|d| d.kind == ActorKind::Npc).collect();
    let animated = npcs
        .iter()
        .filter(|d| d.skeleton.is_some() && d.animations.is_some())
        .count();
    let _ = writeln!(out, "### Skeletons and animations\n");
    let _ = writeln!(
        out,
        "Skeletons: {} paths, {} converted ({} distinct files), {} failed. Animations: {} \
         paths, {} converted ({} distinct files), {} failed. NPCs with a skeleton and both a \
         stand and a walk: {} of {}.\n",
        manifest.skeletons.len(),
        converted(&manifest.skeletons),
        manifest.skeleton_info.len(),
        manifest.skeletons.len() - converted(&manifest.skeletons),
        manifest.animations.len(),
        converted(&manifest.animations),
        manifest.animation_info.len(),
        manifest.animations.len() - converted(&manifest.animations),
        animated,
        npcs.len()
    );
    let rows: Vec<Vec<String>> = manifest
        .skeletons
        .iter()
        .filter_map(|(path, entry)| {
            let info = manifest.skeleton_info.get(entry.converted_hash()?)?;
            let users = docs
                .iter()
                .filter(|d| d.skeleton_path.as_deref() == Some(path.as_str()))
                .count();
            Some(vec![
                format!("`{path}`"),
                info.bones.to_string(),
                info.depth.to_string(),
                users.to_string(),
            ])
        })
        .collect();
    if !rows.is_empty() {
        table(out, &["Skeleton", "Bones", "Depth", "Actors"], &rows);
    }
    let rows: Vec<Vec<String>> = manifest
        .animations
        .iter()
        .filter_map(|(path, entry)| {
            let info = manifest.animation_info.get(entry.converted_hash()?)?;
            Some(vec![
                format!("`{path}`"),
                format!("{:.3}", info.duration),
                info.tracks.to_string(),
                if info.looping { "yes" } else { "no" }.to_owned(),
                info.text_keys.to_string(),
            ])
        })
        .collect();
    if !rows.is_empty() {
        table(
            out,
            &[
                "Animation",
                "Duration (s)",
                "Tracks",
                "Looping",
                "Text keys",
            ],
            &rows,
        );
    }
    let failed: Vec<Vec<String>> = [
        ("skeleton", &manifest.skeletons),
        ("animation", &manifest.animations),
    ]
    .into_iter()
    .flat_map(|(kind, entries)| {
        entries
            .iter()
            .filter(|(_, e)| e.converted_hash().is_none())
            .map(move |(path, e)| {
                vec![
                    kind.to_owned(),
                    format!("`{path}`"),
                    status_name(e.status).to_owned(),
                    e.reason.clone().unwrap_or_default(),
                ]
            })
    })
    .collect();
    if !failed.is_empty() {
        table(out, &["Kind", "Path", "Status", "Reason"], &failed);
    }
}

/// The actors section: resolution counts, parts by role, appearances.
fn write_actors(out: &mut String, mir: &Mir) -> Result<(), MirError> {
    let mut docs: Vec<ActorDoc> = Vec::new();
    for entry in mir.actors() {
        if let Some(doc) = mir.actor(&entry.form)? {
            docs.push(doc);
        }
    }
    let npcs = docs.iter().filter(|d| d.kind == ActorKind::Npc).count();
    let complete = |d: &ActorDoc| d.notes.is_empty() && d.parts.iter().all(|p| p.mesh.is_some());
    let full = docs.iter().filter(|d| complete(d)).count();
    let looks: BTreeSet<&str> = docs.iter().map(|d| d.appearance.as_str()).collect();
    let npc_looks: BTreeSet<&str> = docs
        .iter()
        .filter(|d| d.kind == ActorKind::Npc)
        .map(|d| d.appearance.as_str())
        .collect();
    let _ = writeln!(out, "## Actors\n");
    let _ = writeln!(
        out,
        "Placed actor bases in the slice: {} ({} NPCs, {} creatures), resolved through \
         templates and leveled lists; {} resolved fully (every link resolved, every part \
         converted). {} distinct appearances, {} of them among NPCs.\n",
        docs.len(),
        npcs,
        docs.len() - npcs,
        full,
        looks.len(),
        npc_looks.len()
    );
    let roles = [
        (PartRole::Body, "body"),
        (PartRole::Outfit, "outfit"),
        (PartRole::Head, "head"),
        (PartRole::Hair, "hair"),
    ];
    let rows: Vec<Vec<String>> = roles
        .iter()
        .map(|(role, name)| {
            let parts: Vec<_> = docs
                .iter()
                .flat_map(|d| &d.parts)
                .filter(|p| p.role == *role)
                .collect();
            let missing = parts.iter().filter(|p| p.mesh.is_none()).count();
            vec![
                (*name).to_owned(),
                parts.len().to_string(),
                (parts.len() - missing).to_string(),
                missing.to_string(),
            ]
        })
        .collect();
    table(out, &["Part", "Listed", "Converted", "Missing"], &rows);
    let without = |role: PartRole| {
        docs.iter()
            .filter(|d| d.kind == ActorKind::Npc && !d.parts.iter().any(|p| p.role == role))
            .count()
    };
    let _ = writeln!(
        out,
        "NPCs without a body part: {}; without a head: {}; without hair (bald, or covered by \
         the outfit): {}; without an outfit: {}.\n",
        without(PartRole::Body),
        without(PartRole::Head),
        without(PartRole::Hair),
        without(PartRole::Outfit)
    );
    write_actor_stores(out, mir, &docs);
    let limit = 100;
    let rows: Vec<Vec<String>> = docs
        .iter()
        .take(limit)
        .map(|d| {
            vec![
                format!("`{}`", d.editor_id.as_deref().unwrap_or("")),
                format!("`{}`", d.form),
                match d.kind {
                    ActorKind::Npc => "npc",
                    ActorKind::Creature => "creature",
                }
                .to_owned(),
                if d.female { "f" } else { "m" }.to_owned(),
                d.parts.len().to_string(),
                d.parts
                    .iter()
                    .filter(|p| p.mesh.is_none())
                    .count()
                    .to_string(),
                d.template_chains.len().to_string(),
                d.notes.len().to_string(),
            ]
        })
        .collect();
    if !rows.is_empty() {
        table(
            out,
            &[
                "Actor",
                "Form",
                "Kind",
                "Sex",
                "Parts",
                "Missing",
                "Templated groups",
                "Notes",
            ],
            &rows,
        );
        if docs.len() > limit {
            let _ = writeln!(out, "... and {} more.\n", docs.len() - limit);
        }
    }
    Ok(())
}

/// Writes the report of an opened MIR directory.
///
/// # Errors
///
/// When `records.sqlite` or a cell document cannot be read.
pub fn report(mir: &Mir, params: &ReportParams) -> Result<String, MirError> {
    let manifest = mir.manifest();
    let records: Records = mir.records()?;
    let mut exteriors = Vec::new();
    for key in mir.cells() {
        if let Some(doc) = mir.cell(&key.worldspace, key.x, key.y)? {
            exteriors.push(doc);
        }
    }
    let mut interiors = Vec::new();
    for entry in mir.interiors() {
        if let Some(doc) = mir.interior_by_form(&entry.form)? {
            interiors.push(doc);
        }
    }
    let top = params.top.max(1);
    let mut out = String::new();
    let slice = &manifest.slice;

    let _ = writeln!(out, "# MIR report\n");
    let _ = writeln!(
        out,
        "Slice `{}`: worldspace `{}`, cells x {}..={}, y {}..={}, interior depth {}. \
         Schema version {}; raw store `{}`.\n",
        slice.name,
        slice.worldspace,
        slice.x.start(),
        slice.x.end(),
        slice.y.start(),
        slice.y.end(),
        slice.interior_depth,
        manifest.schema_version,
        manifest.raw_store.as_str(),
    );
    let plugin_rows: Vec<Vec<String>> = manifest
        .plugins
        .iter()
        .map(|p| {
            vec![
                format!("`{}`", p.name),
                p.size.to_string(),
                format!("`{}`", p.sha256),
            ]
        })
        .collect();
    table(&mut out, &["Plugin", "Bytes", "SHA-256"], &plugin_rows);
    let archives: Vec<String> = manifest
        .archives
        .iter()
        .map(|a| match &a.error {
            Some(error) => format!("`{}` (did not open: {error})", a.name),
            None => format!("`{}`", a.name),
        })
        .collect();
    let _ = writeln!(out, "Archive order: {}.\n", archives.join(", "));

    // Records.
    let counts = records.record_counts()?;
    let total: u64 = counts.values().sum();
    let _ = writeln!(out, "## Records\n");
    let _ = writeln!(out, "{total} records in {} kinds.\n", counts.len());
    let rows: Vec<Vec<String>> = rank(&counts)
        .into_iter()
        .map(|(k, v)| vec![format!("`{k}`"), v.to_string()])
        .collect();
    table(&mut out, &["Kind", "Records"], &rows);

    // References.
    let corpus_refs = records.reference_counts()?;
    let mut slice_ext: BTreeMap<String, u64> = BTreeMap::new();
    let mut slice_int: BTreeMap<String, u64> = BTreeMap::new();
    for (docs, counts) in [(&exteriors, &mut slice_ext), (&interiors, &mut slice_int)] {
        for doc in docs.iter() {
            for r in &doc.references {
                let kind = r.base_kind.clone().unwrap_or_else(|| "(none)".to_owned());
                *counts.entry(kind).or_insert(0) += 1;
            }
        }
    }
    let kinds: BTreeSet<&String> = corpus_refs
        .keys()
        .chain(slice_ext.keys())
        .chain(slice_int.keys())
        .collect();
    let mut ref_rows: Vec<(u64, String, Vec<String>)> = kinds
        .into_iter()
        .map(|k| {
            let get = |m: &BTreeMap<String, u64>| m.get(k).copied().unwrap_or(0);
            (
                get(&corpus_refs),
                k.clone(),
                vec![
                    format!("`{k}`"),
                    get(&corpus_refs).to_string(),
                    get(&slice_ext).to_string(),
                    get(&slice_int).to_string(),
                ],
            )
        })
        .collect();
    ref_rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let _ = writeln!(out, "## References by base kind\n");
    let _ = writeln!(
        out,
        "{} placed references in the plugin list; {} in the slice's {} exterior cells \
         (worldspace persistent references assigned by position), {} in its {} interiors.\n",
        corpus_refs.values().sum::<u64>(),
        slice_ext.values().sum::<u64>(),
        exteriors.len(),
        slice_int.values().sum::<u64>(),
        interiors.len(),
    );
    table(
        &mut out,
        &["Base kind", "Corpus", "Slice exteriors", "Slice interiors"],
        &ref_rows.into_iter().map(|r| r.2).collect::<Vec<_>>(),
    );

    // Cells.
    let _ = writeln!(out, "## Cells\n");
    let navmeshes: usize = exteriors
        .iter()
        .chain(&interiors)
        .map(|d| d.navmeshes.len())
        .sum();
    let triangles: usize = exteriors
        .iter()
        .chain(&interiors)
        .flat_map(|d| &d.navmeshes)
        .map(|n| n.triangles.len())
        .sum();
    let with_land = exteriors.iter().filter(|d| d.land.is_some()).count();
    let teleports = exteriors
        .iter()
        .chain(&interiors)
        .flat_map(|d| &d.references)
        .filter(|r| r.teleport.is_some())
        .count();
    let triggers: Vec<&Reference> = exteriors
        .iter()
        .chain(&interiors)
        .flat_map(|d| &d.references)
        .filter(|r| r.trigger_volume)
        .collect();
    let stand_ins = triggers.iter().filter(|r| r.model.is_some()).count();
    let _ = writeln!(
        out,
        "- exterior cells: {} ({} with land)\n- interior cells: {}\n- navmeshes: {} with {} triangles\n- teleport doors: {}\n- trigger volumes: {} ({} with a stand-in model, not drawn){}\n",
        exteriors.len(),
        with_land,
        interiors.len(),
        navmeshes,
        triangles,
        teleports,
        triggers.len(),
        stand_ins,
        default_land_line(manifest, &exteriors),
    );
    if !manifest.interiors.is_empty() {
        let rows: Vec<Vec<String>> = manifest
            .interiors
            .iter()
            .map(|i| {
                vec![
                    format!("`{}`", i.id),
                    format!("`{}`", i.form),
                    i.depth.to_string(),
                ]
            })
            .collect();
        table(&mut out, &["Interior", "Form", "Depth"], &rows);
    }

    // Meshes and textures.
    let _ = writeln!(out, "## Meshes and textures\n");
    let mut asset_rows = Vec::new();
    for (label, entries, converted) in [
        ("Meshes", &manifest.meshes, manifest.mesh_info.len()),
        ("Textures", &manifest.textures, manifest.texture_info.len()),
    ] {
        let count = |s: AssetStatus| entries.values().filter(|e| e.status == s).count();
        asset_rows.push(vec![
            label.to_owned(),
            entries.len().to_string(),
            converted.to_string(),
            count(AssetStatus::Missing).to_string(),
            count(AssetStatus::Unreadable).to_string(),
            count(AssetStatus::Undecodable).to_string(),
        ]);
    }
    table(
        &mut out,
        &[
            "",
            "Paths referenced",
            "Distinct files converted",
            "Missing",
            "Unreadable",
            "Undecodable",
        ],
        &asset_rows,
    );
    let mut skipped: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut skinned = 0u64;
    for info in manifest.mesh_info.values() {
        skinned += u64::from(info.skinned_primitives);
        for (block, count) in &info.skipped_blocks {
            let e = skipped.entry(block.clone()).or_insert((0, 0));
            e.0 += u64::from(*count);
            e.1 += 1;
        }
    }
    let _ = writeln!(
        out,
        "{} skinned primitives across the converted meshes.\n",
        skinned
    );
    let _ = writeln!(out, "### Block types the mesh parser skipped\n");
    if skipped.is_empty() {
        let _ = writeln!(out, "None.\n");
    } else {
        let mut list: Vec<(String, (u64, u64))> = skipped.into_iter().collect();
        list.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
        let rows: Vec<Vec<String>> = list
            .into_iter()
            .map(|(k, (blocks, meshes))| {
                vec![format!("`{k}`"), blocks.to_string(), meshes.to_string()]
            })
            .collect();
        table(&mut out, &["Block type", "Blocks", "Meshes"], &rows);
    }

    // NPCs.
    let appearances = records.npc_appearances()?;
    let by_form: BTreeMap<String, (String, String)> = appearances
        .iter()
        .map(|a| (a.form.to_string(), (a.appearance.clone(), a.outfit.clone())))
        .collect();
    let distinct = |forms: &mut dyn Iterator<Item = &(String, String)>| {
        let mut looks = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        for (look, outfit) in forms {
            looks.insert(look.clone());
            pairs.insert((look.clone(), outfit.clone()));
        }
        (looks.len(), pairs.len())
    };
    let (corpus_looks, corpus_pairs) = distinct(&mut by_form.values());
    let mut slice_npcs: BTreeSet<String> = BTreeSet::new();
    let mut slice_npc_refs = 0usize;
    for doc in exteriors.iter().chain(&interiors) {
        for r in doc.references.iter().filter(|r| r.kind == RefKind::Npc) {
            slice_npc_refs += 1;
            slice_npcs.insert(r.base.to_string());
        }
    }
    let slice_known: Vec<&(String, String)> =
        slice_npcs.iter().filter_map(|f| by_form.get(f)).collect();
    let (slice_looks, slice_pairs) = distinct(&mut slice_known.into_iter());
    let _ = writeln!(out, "## NPCs and appearance clusters\n");
    let _ = writeln!(
        out,
        "An appearance signature is (race, sex, hair, hair colour, eyes, hash of the FaceGen \
         geometry, asymmetry and texture blobs); an outfit is the sorted set of armour items \
         in the NPC's inventory. Templates and leveled lists are not followed here; the \
         actors section below follows them.\n"
    );
    table(
        &mut out,
        &[
            "",
            "NPC records",
            "Appearance signatures",
            "(Appearance, outfit) pairs",
        ],
        &[
            vec![
                "Corpus".to_owned(),
                appearances.len().to_string(),
                corpus_looks.to_string(),
                corpus_pairs.to_string(),
            ],
            vec![
                format!("Slice ({slice_npc_refs} NPC references)"),
                slice_npcs.len().to_string(),
                slice_looks.to_string(),
                slice_pairs.to_string(),
            ],
        ],
    );

    write_actors(&mut out, mir)?;

    // Script commands.
    let scripts = records.scripts()?;
    let embedded = records.embedded_scripts()?;
    let editor_ids: BTreeSet<String> = records.editor_ids()?.into_iter().collect();
    let all_variables: BTreeSet<String> = scripts
        .iter()
        .flat_map(|s| s.variables.iter().map(|v| v.to_ascii_lowercase()))
        .collect();
    let mut uses: BTreeMap<String, u64> = BTreeMap::new();
    let mut users: BTreeMap<String, u64> = BTreeMap::new();
    let mut sources = 0usize;
    let empty = BTreeSet::new();
    let mut count_source = |source: &str, locals: &BTreeSet<String>| {
        sources += 1;
        let found = commands(source, locals, &all_variables, &editor_ids);
        let distinct: BTreeSet<&String> = found.iter().collect();
        for word in &distinct {
            *users.entry((*word).clone()).or_insert(0) += 1;
        }
        for word in found {
            *uses.entry(word).or_insert(0) += 1;
        }
    };
    for script in &scripts {
        if let Some(source) = &script.source {
            let locals: BTreeSet<String> = script
                .variables
                .iter()
                .map(|v| v.to_ascii_lowercase())
                .collect();
            count_source(source, &locals);
        }
    }
    for (_, _, source) in &embedded {
        count_source(source, &empty);
    }
    let _ = writeln!(out, "## Script commands\n");
    let _ = writeln!(
        out,
        "{} script sources ({} script records, {} result scripts), {} distinct command \
         identifiers. Identifiers are counted after dropping comments, strings, numbers, \
         keywords, block types, declared variables (and, after a dot, any script's \
         variables) and the editor ids of records.\n",
        sources,
        scripts.iter().filter(|s| s.source.is_some()).count(),
        embedded.len(),
        uses.len()
    );
    let rows: Vec<Vec<String>> = rank(&uses)
        .into_iter()
        .take(top)
        .enumerate()
        .map(|(i, (word, n))| {
            vec![
                (i + 1).to_string(),
                format!("`{word}`"),
                n.to_string(),
                users.get(&word).copied().unwrap_or(0).to_string(),
            ]
        })
        .collect();
    table(&mut out, &["Rank", "Command", "Uses", "Sources"], &rows);

    let functions = records.condition_function_counts()?;
    let _ = writeln!(out, "## Condition functions\n");
    let _ = writeln!(
        out,
        "{} conditions using {} distinct function indexes.\n",
        functions.values().sum::<u64>(),
        functions.len()
    );
    let rows: Vec<Vec<String>> = rank(&functions)
        .into_iter()
        .take(top)
        .enumerate()
        .map(|(i, (f, n))| vec![(i + 1).to_string(), f.to_string(), n.to_string()])
        .collect();
    table(&mut out, &["Rank", "Function index", "Uses"], &rows);

    // Problems.
    let bad = records.bad_records()?;
    let _ = writeln!(out, "## Undecodable records and missing files\n");
    let _ = writeln!(out, "{} undecodable records.\n", bad.len());
    let limit = 100;
    if !bad.is_empty() {
        let rows: Vec<Vec<String>> = bad
            .iter()
            .take(limit)
            .map(|b| {
                vec![
                    format!("`{}`", b.plugin),
                    format!("{:#x}", b.offset),
                    format!("`{}`", b.sig),
                    b.form
                        .clone()
                        .map_or_else(String::new, |f| format!("`{f}`")),
                    b.error.clone(),
                ]
            })
            .collect();
        table(
            &mut out,
            &["Plugin", "Offset", "Kind", "Form", "Error"],
            &rows,
        );
        if bad.len() > limit {
            let _ = writeln!(out, "... and {} more.\n", bad.len() - limit);
        }
    }
    let _ = writeln!(out, "{} slice extraction issues.\n", manifest.issues.len());
    if !manifest.issues.is_empty() {
        let rows: Vec<Vec<String>> = manifest
            .issues
            .iter()
            .take(limit)
            .map(|i| {
                vec![
                    format!("`{}`", i.subject),
                    i.sig.clone().map_or_else(String::new, |s| format!("`{s}`")),
                    i.offset.map_or_else(String::new, |o| format!("{o:#x}")),
                    i.error.clone(),
                ]
            })
            .collect();
        table(&mut out, &["Subject", "Kind", "Offset", "Error"], &rows);
        if manifest.issues.len() > limit {
            let _ = writeln!(out, "... and {} more.\n", manifest.issues.len() - limit);
        }
    }
    let failed: Vec<Vec<String>> = manifest
        .meshes
        .iter()
        .map(|(p, e)| ("mesh", p, e))
        .chain(manifest.textures.iter().map(|(p, e)| ("texture", p, e)))
        .filter(|(_, _, e)| e.status != AssetStatus::Converted)
        .map(|(kind, path, e)| {
            vec![
                kind.to_owned(),
                format!("`{path}`"),
                format!("{:?}", e.status).to_lowercase(),
                e.reason.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let _ = writeln!(out, "{} files missing or not converted.\n", failed.len());
    if !failed.is_empty() {
        let shown: Vec<Vec<String>> = failed.iter().take(limit).cloned().collect();
        table(&mut out, &["Asset", "Path", "Status", "Reason"], &shown);
        if failed.len() > limit {
            let _ = writeln!(out, "... and {} more.\n", failed.len() - limit);
        }
    }

    // FID budget.
    let tile = params.tile_units.max(1.0);
    let _ = writeln!(out, "## Wall and scenery FID budget\n");
    let _ = writeln!(
        out,
        "Estimator: every distinct (mesh hash, yaw rounded to the degree, scale rounded to \
         0.01) among the slice's exterior references whose base is one of {} needs its own \
         art. Its footprint is the mesh's XY bounds times the scale, an oriented rectangle of \
         `w × h` floor squares ({} source units each). The hexes it covers are estimated as \
         `round((2w + 1)(2h + 1))`, the expected count of half-square cells (a hex is a \
         quarter of a floor square in area) that a `w × h` rectangle touches at a random \
         offset. One hex counts as one scenery entry; more counts as one wall-strip entry per \
         hex. Each list holds at most {} entries.\n",
        SCENERY_KINDS
            .iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", "),
        tile,
        ART_LIST_LIMIT
    );
    let _ = writeln!(out, "### Whole slice\n");
    let whole = budget(mir, exteriors.iter(), tile, None, 1.0);
    write_budget(&mut out, &whole, top);
    write_coarse(&mut out, &budget(mir, exteriors.iter(), tile, None, 60.0));
    let centre = [
        (*slice.x.start() as f32 + *slice.x.end() as f32 + 1.0) / 2.0 * CELL_UNITS,
        (*slice.y.start() as f32 + *slice.y.end() as f32 + 1.0) / 2.0 * CELL_UNITS,
    ];
    let half = MAP_SQUARES * tile / 2.0;
    let _ = writeln!(
        out,
        "### One map window\n\n{} × {} floor squares ({} units square) centred on the slice at ({}, {}).\n",
        MAP_SQUARES,
        MAP_SQUARES,
        MAP_SQUARES * tile,
        centre[0],
        centre[1]
    );
    let window = budget(mir, exteriors.iter(), tile, Some((centre, half)), 1.0);
    write_budget(&mut out, &window, top);
    let coarse = budget(mir, exteriors.iter(), tile, Some((centre, half)), 60.0);
    write_coarse(&mut out, &coarse);
    Ok(out)
}

/// The report line on land layers drawn with the default land texture,
/// with a leading line break; empty when there are none, so that a report
/// without them reads as before.
fn default_land_line(manifest: &crate::model::Manifest, exteriors: &[CellDoc]) -> String {
    let layers: Vec<&crate::model::LandLayer> = exteriors
        .iter()
        .filter_map(|d| d.land.as_ref())
        .flat_map(|l| &l.layers)
        .collect();
    let mut reasons: BTreeMap<crate::model::LandDefault, usize> = BTreeMap::new();
    for reason in layers.iter().filter_map(|l| l.defaulted) {
        *reasons.entry(reason).or_default() += 1;
    }
    let total: usize = reasons.values().sum();
    if total == 0 {
        return String::new();
    }
    let missing = layers
        .iter()
        .filter(|l| l.is_defaulted() && l.texture_hash.is_none())
        .count();
    let parts: Vec<String> = reasons
        .iter()
        .map(|(reason, n)| {
            let what = match reason {
                crate::model::LandDefault::NoBaseLayer => "quadrants without a base layer",
                crate::model::LandDefault::NullTexture => "null land textures",
                crate::model::LandDefault::UnknownTexture => "forms that are not land textures",
                crate::model::LandDefault::NoTextureSet => {
                    "land textures without a diffuse texture"
                }
            };
            format!("{n} {what}")
        })
        .collect();
    format!(
        "\n- land layers drawn with the default land texture `{}`: {total} of {} ({}){}",
        manifest.default_land_texture.as_deref().unwrap_or("?"),
        layers.len(),
        parts.join(", "),
        if missing > 0 {
            format!("; the texture is missing, so {missing} of them have none")
        } else {
            String::new()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_commands_drop_keywords_variables_and_ids() {
        let source = "scn TestScript\nshort done\nref target\nbegin OnActivate\n  if done == 0 ; first time\n    set done to 1\n    ShowMessage SomeMessage\n    target.MoveTo Marker 10\n    SomeQuest.counter\n  endif\n  Activate \"text Activate\"\nend\n";
        let ids: BTreeSet<String> = ["somemessage", "marker", "somequest", "testscript"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let vars: BTreeSet<String> = ["counter".to_owned()].into_iter().collect();
        let found = commands(source, &BTreeSet::new(), &vars, &ids);
        assert_eq!(found, ["showmessage", "moveto", "activate"]);
    }

    #[test]
    fn hex_estimate() {
        assert_eq!(hexes_covered(0.0, 0.0), 1);
        assert_eq!(hexes_covered(0.1, 0.1), 1);
        assert_eq!(hexes_covered(1.0, 1.0), 9);
        assert_eq!(hexes_covered(5.0, 0.1), 13);
    }

    #[test]
    fn tables_are_well_formed() {
        let mut out = String::new();
        table(
            &mut out,
            &["A", "B"],
            &[vec!["x|y".to_owned(), "1".to_owned()]],
        );
        assert_eq!(out, "| A | B |\n| --- | ---: |\n| x\\|y | 1 |\n\n");
    }
}
