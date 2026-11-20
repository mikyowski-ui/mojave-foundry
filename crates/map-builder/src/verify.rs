//! [`verify_layer`]: every file of a layer parsed back through
//! `vault-formats`, over the base files with the layer mounted on top.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use vault_formats::fallout2::Diagnostics;
use vault_formats::fallout2::maps::MapsTable;
use vault_formats::fid::ART_DIRECTORIES;
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::mod_manifest::{MANIFEST, MAP_NUMBERS, ModManifest, ModRegistry, README};
use vault_formats::pro::{Category, PrototypeData, PrototypeKind, PrototypeLookup};
use vault_formats::{
    ArtCatalog, ByteSource, Fid, FrmArchive, GameProfile, LayeredVfs, LocalPrototypeCatalog,
    LstKind, LstTable, MAX_ASSET_BYTES, MemoryDir, MsgCatalog, Prototype, Vfs, VfsPath,
};

use crate::BuildError;
use crate::assemble::{EXIT_GRID_PID, MAPS_TXT};

/// One map that parsed back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifiedMap {
    pub file: String,
    pub number: u32,
    pub entry_hex: i32,
    pub objects: usize,
    pub exit_grids: usize,
}

/// What [`verify_layer`] checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub frm: usize,
    pub art_lists: usize,
    pub prototype_lists: usize,
    pub prototypes: usize,
    pub messages: usize,
    pub maps: Vec<VerifiedMap>,
    /// Object and floor-tile art identifiers resolved to a file.
    pub fids_resolved: usize,
    /// Whether the layer carries `data/maps.txt` and it was checked.
    pub maps_txt: bool,
    /// Critter FRMs checked for six directions.
    pub critter_frm: usize,
    /// New `art/critters/critters.lst` rows whose stand and walk FIDs
    /// resolved to files of the layer.
    pub critter_rows: usize,
    /// Critter prototypes checked, with their names.
    pub critter_prototypes: usize,
    /// Critter objects checked on the maps.
    pub critters: usize,
}

/// `art/critters/critters.lst`.
const CRITTER_LIST: &str = emit::CRITTER_LIST_PATH;

/// Whether every one of the six directions resolves all frames.
fn six_directions(frm: &FrmArchive) -> bool {
    (0..6).all(|d| (0..frm.frame_count()).all(|f| frm.frame(d, f).is_some()))
}

fn tree(
    files: impl IntoIterator<Item = (String, Vec<u8>)>,
    what: &str,
) -> Result<MemoryDir, BuildError> {
    let entries = files
        .into_iter()
        .map(|(path, bytes)| {
            ByteSource::new(bytes)
                .map(|source| (path.clone(), source))
                .map_err(|e| BuildError::verify(path, e))
        })
        .collect::<Result<Vec<_>, _>>()?;
    MemoryDir::new(entries).map_err(|e| BuildError::verify(what, e))
}

/// Parse back every file of `layer` mounted over `base_files` (the same
/// map [`crate::BuildInput::base_files`] took). The checks, in order:
///
/// 1. `vault-mod.toml`: `ModManifest::parse` of the written text equals
///    the layer's manifest; every `[[maps]]` file is in the layer, its
///    number in 66 to 65535; `ModRegistry::from_vfs` over the mounted
///    layer registers exactly those numbers and files.
/// 2. Every `.frm`: `FrmArchive::parse`, with frame 0 of direction 0.
/// 3. Every `.lst`: `LstTable::parse` with the kind its path selects
///    (`LstKind::for_path`); an art list holds at most 4,096 entries.
/// 4. Every `.msg`: `MsgCatalog::parse_strict`.
/// 5. Every `.pro`: `Prototype::parse_for` (Fallout 2) round-trips to the
///    same bytes, and `LocalPrototypeCatalog` (Fallout 2, over the mounted
///    layer) resolves its PID to it through its list position.
/// 6. Every `.map`: `MapFile::parse_for` (Fallout 2) through that catalog,
///    version 20, no diagnostics; the header name is the file name in
///    capitals and the header number its registered number; every object
///    art FID and every floor tile other than the blank 1 resolves through
///    `ArtCatalog::load` (the mounted art lists) to a list entry, and an
///    entry this layer added to a file the layer holds; every exit grid
///    leads to a registered map, onto a hex of the grid, at elevation 0
///    to 2.
/// 7. `data/maps.txt`, when present: `MapsTable::parse`, and every
///    registered map has the record of its number naming its file.
/// 8. No other file is in the layer.
///
/// Critters (contracts section 10), on top of the above:
///
/// - every `art/critters/*.frm` resolves all its frames in each of the six
///   directions;
/// - every row the layer's `art/critters/critters.lst` adds past the base
///   list resolves, from its stand FID (animation 0) and its walk FID
///   (animation 1), through `ArtCatalog::load` to a `<stem>aa.frm` and
///   `<stem>ab.frm` the layer holds, each with six directions;
/// - every `proto/critters/*.pro` is Fallout 2 critter data that
///   `LocalPrototypeCatalog` resolves as a critter at its list position,
///   with stand art and maximum hit points above 0, and its name and
///   description (message base and base + 1) are in the mounted
///   `pro_crit.msg`;
/// - every critter object on a map has critter art whose FID resolves (as
///   in 6), a PID the catalog resolves to a critter prototype, current hit
///   points above 0 and an orientation of 0 to 5; and no object carries
///   critter art or a critter PID without a critter record.
pub fn verify_layer(
    layer: &emit::ModLayer,
    base_files: &BTreeMap<String, Vec<u8>>,
) -> Result<VerifyReport, BuildError> {
    let mut report = VerifyReport::default();
    // 1. The manifest.
    let text = layer
        .manifest_text()
        .map_err(|e| BuildError::verify(MANIFEST, e))?;
    let manifest = ModManifest::parse(&text).map_err(|e| BuildError::verify(MANIFEST, e))?;
    if &manifest != layer.manifest() {
        return Err(BuildError::verify(
            MANIFEST,
            "parses into a different manifest",
        ));
    }
    let mut registered: BTreeMap<String, u32> = BTreeMap::new();
    for map in &manifest.maps {
        let path = format!("maps/{}", map.file);
        if !layer.files().contains_key(&path) {
            return Err(BuildError::verify(
                MANIFEST,
                format!("registers {path}, which the layer lacks"),
            ));
        }
        if !MAP_NUMBERS.contains(&map.number) {
            return Err(BuildError::verify(
                MANIFEST,
                format!("map number {} is out of range", map.number),
            ));
        }
        registered.insert(path, map.number);
    }
    let numbers: BTreeSet<u32> = registered.values().copied().collect();

    let base = tree(base_files.clone(), "base files")?;
    let mut layer_files: Vec<(String, Vec<u8>)> = layer
        .files()
        .iter()
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    layer_files.push((MANIFEST.to_owned(), text.into_bytes()));
    let mounted = tree(layer_files, "layer files")?;
    let vfs = LayeredVfs::new(
        Arc::new(base),
        vec![vault_formats::ModLayer::from_tree("map-builder", mounted)],
    );
    let registry = ModRegistry::from_vfs(&vfs).map_err(|e| BuildError::verify(MANIFEST, e))?;
    let from_registry: BTreeMap<String, u32> = registry
        .maps
        .values()
        .map(|map| (format!("maps/{}", map.file), map.number))
        .collect();
    if from_registry != registered {
        return Err(BuildError::verify(
            MANIFEST,
            "the mounted registry disagrees with the manifest",
        ));
    }

    // Base list lengths: entries below them are the install's art.
    let mut base_len: BTreeMap<u8, usize> = BTreeMap::new();
    for (category, directory) in ART_DIRECTORIES.iter().enumerate() {
        let path = format!("art/{directory}/{directory}.lst");
        if let Some(bytes) = base_files.get(&path) {
            let table =
                LstTable::parse(bytes, LstKind::Art).map_err(|e| BuildError::verify(&path, e))?;
            base_len.insert(category as u8, table.entries().len());
        }
    }

    // 2-5, 7, 8: file by file.
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let mut map_paths = Vec::new();
    for (path, bytes) in layer.files() {
        let vfs_path = VfsPath::new(path).map_err(|e| BuildError::verify(path, e))?;
        match vfs_path.extension() {
            Some("frm") => {
                let frm = FrmArchive::parse(bytes).map_err(|e| BuildError::verify(path, e))?;
                if frm.frame(0, 0).is_none() {
                    return Err(BuildError::verify(path, "has no frame"));
                }
                if path.starts_with("art/critters/") {
                    if !six_directions(&frm) {
                        return Err(BuildError::verify(
                            path,
                            "a critter FRM lacks frames in one of its six directions",
                        ));
                    }
                    report.critter_frm += 1;
                }
                report.frm += 1;
            }
            Some("lst") => {
                let kind = LstKind::for_path(&vfs_path);
                let table =
                    LstTable::parse(bytes, kind).map_err(|e| BuildError::verify(path, e))?;
                match kind {
                    LstKind::Art => {
                        if table.entries().len() > emit::FID_INDEX_CAP {
                            return Err(BuildError::verify(path, "holds more than 4096 entries"));
                        }
                        report.art_lists += 1;
                    }
                    LstKind::Prototype => report.prototype_lists += 1,
                    _ => return Err(BuildError::verify(path, "is not an art or prototype list")),
                }
            }
            Some("msg") => {
                MsgCatalog::parse_strict(bytes).map_err(|e| BuildError::verify(path, e))?;
                report.messages += 1;
            }
            Some("pro") => {
                let prototype = Prototype::parse_for(bytes, GameProfile::Fallout2)
                    .map_err(|e| BuildError::verify(path, e))?;
                if prototype.write() != *bytes {
                    return Err(BuildError::verify(
                        path,
                        "does not write back to the same bytes",
                    ));
                }
                let info = catalog
                    .lookup(prototype.pid)
                    .map_err(|e| BuildError::verify(path, e))?;
                if path.starts_with("proto/critters/") {
                    check_critter_prototype(&vfs, path, &prototype, info.kind)?;
                    report.critter_prototypes += 1;
                }
                report.prototypes += 1;
            }
            Some("map") => map_paths.push(path.clone()),
            Some("txt") if path == MAPS_TXT => {
                let table = MapsTable::parse(bytes, &mut Diagnostics::default())
                    .map_err(|e| BuildError::verify(path, e))?;
                for (map, number) in &registered {
                    let record = table.get(*number as i32).ok_or_else(|| {
                        BuildError::verify(path, format!("has no record {number} for {map}"))
                    })?;
                    if &record.path() != map {
                        return Err(BuildError::verify(
                            path,
                            format!("record {number} names {}, not {map}", record.path()),
                        ));
                    }
                }
                report.maps_txt = true;
            }
            _ => return Err(BuildError::verify(path, "is not a file this layer writes")),
        }
    }

    // 6. Critter rows, then maps.
    let art = ArtCatalog::load(&vfs).map_err(|e| BuildError::verify("art lists", e))?;
    if let Some(bytes) = layer.files().get(CRITTER_LIST) {
        let table = LstTable::parse(bytes, LstKind::Art)
            .map_err(|e| BuildError::verify(CRITTER_LIST, e))?;
        let first = base_len.get(&1).copied().unwrap_or(0);
        for row in first..table.entries().len() {
            let row = u16::try_from(row)
                .map_err(|_| BuildError::verify(CRITTER_LIST, "a row past 65535"))?;
            for animation in [0, 1] {
                let fid = Fid::new(1, row, animation, 0, 0);
                let target = art.path(fid).map_err(|e| {
                    BuildError::verify(CRITTER_LIST, format!("row {row} FID {:#010x}: {e}", fid.0))
                })?;
                let file = layer.files().get(&target).ok_or_else(|| {
                    BuildError::verify(
                        CRITTER_LIST,
                        format!("row {row} names {target}, which the layer lacks"),
                    )
                })?;
                let frm = FrmArchive::parse(file).map_err(|e| BuildError::verify(&target, e))?;
                if !six_directions(&frm) {
                    return Err(BuildError::verify(&target, "lacks one of six directions"));
                }
            }
            report.critter_rows += 1;
        }
    }
    let mut present: BTreeSet<String> = BTreeSet::new();
    let mut resolve = |fid: Fid, at: &str| -> Result<(), BuildError> {
        let target = art
            .path(fid)
            .map_err(|e| BuildError::verify(at, format!("FID {:#010x}: {e}", fid.0)))?;
        let added = fid.base() as usize >= base_len.get(&fid.category()).copied().unwrap_or(0);
        if added && !present.contains(&target) {
            let bytes = layer.files().get(&target).ok_or_else(|| {
                BuildError::verify(
                    at,
                    format!("FID {:#010x} names {target}, which the layer lacks", fid.0),
                )
            })?;
            FrmArchive::parse(bytes).map_err(|e| BuildError::verify(&target, e))?;
            present.insert(target);
        }
        Ok(())
    };
    for path in &map_paths {
        let bytes = &layer.files()[path];
        let map = MapFile::parse_for(bytes, &catalog, GameProfile::Fallout2)
            .map_err(|e| BuildError::verify(path, e))?;
        if map.header.version != 20 {
            return Err(BuildError::verify(
                path,
                format!("version {}", map.header.version),
            ));
        }
        if !map.diagnostics.is_empty() {
            return Err(BuildError::verify(path, map.diagnostics.join("; ")));
        }
        let file = path.trim_start_matches("maps/");
        let name_end = map.header.name.iter().position(|b| *b == 0).unwrap_or(16);
        if !map.header.name[..name_end].eq_ignore_ascii_case(file.as_bytes())
            || map.header.name[..name_end]
                .iter()
                .any(u8::is_ascii_lowercase)
        {
            return Err(BuildError::verify(
                path,
                "the header name is not the file name in capitals",
            ));
        }
        let number = *registered
            .get(path)
            .ok_or_else(|| BuildError::verify(path, "is not registered in the manifest"))?;
        if map.header.map_number != number as i32 {
            return Err(BuildError::verify(
                path,
                format!(
                    "header number {} is registered as {number}",
                    map.header.map_number
                ),
            ));
        }
        let mut verified = VerifiedMap {
            file: file.to_owned(),
            number,
            entry_hex: map.header.entry_hex,
            ..VerifiedMap::default()
        };
        for object in map.objects.elevations.iter().flatten() {
            verified.objects += 1;
            resolve(Fid(object.art_id), path)?;
            report.fids_resolved += 1;
            let fid = Fid(object.art_id);
            let critter_pid = object.pid >> 24 == 1;
            if let ObjectState::Critter(state) = &object.state {
                let at = format!("critter object {}", object.id);
                if fid.category() != 1 || !critter_pid {
                    return Err(BuildError::verify(
                        path,
                        format!(
                            "{at}: FID {:#010x} and PID {:#010x} are not both critter ones",
                            fid.0, object.pid
                        ),
                    ));
                }
                let info = catalog
                    .lookup(object.pid)
                    .map_err(|e| BuildError::verify(path, format!("{at}: {e}")))?;
                if info.kind != PrototypeKind::Critter {
                    return Err(BuildError::verify(
                        path,
                        format!("{at}: PID {:#010x} is not a critter prototype", object.pid),
                    ));
                }
                if state.hp <= 0 {
                    return Err(BuildError::verify(
                        path,
                        format!("{at}: {} hit points; at 0 the critter is dead", state.hp),
                    ));
                }
                if object.orientation > 5 {
                    return Err(BuildError::verify(
                        path,
                        format!("{at}: orientation {}", object.orientation),
                    ));
                }
                report.critters += 1;
            } else if fid.category() == 1 || critter_pid {
                return Err(BuildError::verify(
                    path,
                    format!(
                        "object {} has critter art or PID but no critter record",
                        object.id
                    ),
                ));
            }
            if let ObjectState::Other {
                detail:
                    ObjectDetail::ExitGrid {
                        map: to,
                        hex,
                        elevation,
                        ..
                    },
                ..
            } = &object.state
            {
                if !(EXIT_GRID_PID..EXIT_GRID_PID + 8).contains(&object.pid) {
                    return Err(BuildError::verify(
                        path,
                        "an exit grid with a non-exit-grid PID",
                    ));
                }
                if !u32::try_from(*to).is_ok_and(|to| numbers.contains(&to)) {
                    return Err(BuildError::verify(
                        path,
                        format!("exit grid {} leads to unregistered map {to}", object.id),
                    ));
                }
                if !(0..40_000).contains(hex) || !(0..=2).contains(elevation) {
                    return Err(BuildError::verify(
                        path,
                        format!("exit grid {} leads off the grid", object.id),
                    ));
                }
                verified.exit_grids += 1;
            }
        }
        for tiles in map.tiles.iter().flatten() {
            let mut seen = BTreeSet::new();
            for tile in tiles {
                for index in [tile.floor(), tile.roof()] {
                    if index != 1 && seen.insert(index) {
                        resolve(Fid::new(4, index, 0, 0, 0), path)?;
                        report.fids_resolved += 1;
                    }
                }
            }
        }
        report.maps.push(verified);
    }
    Ok(report)
}

/// A layer's critter prototype: Fallout 2 critter data, resolving as a
/// critter, stand art (category 1, animation 0), maximum hit points above
/// 0, and its name and description in the mounted `pro_crit.msg`.
fn check_critter_prototype(
    vfs: &LayeredVfs,
    path: &str,
    prototype: &Prototype,
    kind: PrototypeKind,
) -> Result<(), BuildError> {
    let PrototypeData::Critter(critter) = &prototype.data else {
        return Err(BuildError::verify(path, "is not critter data"));
    };
    if kind != PrototypeKind::Critter {
        return Err(BuildError::verify(path, "does not resolve as a critter"));
    }
    let fid = Fid(prototype.art_id);
    if fid.category() != 1 || fid.animation() != 0 {
        return Err(BuildError::verify(
            path,
            format!("art {:#010x} is not critter stand art", fid.0),
        ));
    }
    let hit_points = critter.base_stats[emit::proto::stat::MAX_HIT_POINTS];
    if hit_points <= 0 {
        return Err(BuildError::verify(
            path,
            format!("maximum hit points {hit_points}"),
        ));
    }
    let messages_path = emit::proto::message_file_path(Category::Critter, crate::LANGUAGE);
    let bytes = vfs
        .read(&messages_path)
        .map_err(|e| BuildError::verify(&messages_path, e))?;
    let messages =
        MsgCatalog::parse_strict(&bytes).map_err(|e| BuildError::verify(&messages_path, e))?;
    for id in [prototype.message_base, prototype.message_base + 1] {
        if messages.get(id).is_none() {
            return Err(BuildError::verify(
                path,
                format!("message {id} is not in {messages_path}"),
            ));
        }
    }
    Ok(())
}

/// Read a layer folder written by `ModLayer::write` back into memory:
/// its manifest, and every file but the manifest and `README.txt`.
pub fn read_layer(dir: &Path) -> Result<emit::ModLayer, BuildError> {
    let io = |path: &Path, error: std::io::Error| BuildError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    };
    let manifest_path = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| io(&manifest_path, e))?;
    let manifest = ModManifest::parse(&text).map_err(|e| BuildError::verify(MANIFEST, e))?;
    let mut layer =
        emit::ModLayer::from_manifest(manifest).map_err(|e| BuildError::emit(MANIFEST, e))?;
    let mut stack = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(&folder).map_err(|e| io(&folder, e))? {
            let path = entry.map_err(|e| io(&folder, e))?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let relative = path
            .strip_prefix(dir)
            .map_err(|_| BuildError::Io {
                path: path.display().to_string(),
                message: "outside the layer folder".into(),
            })?
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if relative == MANIFEST || relative == README {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| io(&path, e))?;
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(BuildError::Io {
                path: path.display().to_string(),
                message: "larger than an asset may be".into(),
            });
        }
        layer
            .insert(&relative, bytes)
            .map_err(|e| BuildError::emit(relative.clone(), e))?;
    }
    Ok(layer)
}
