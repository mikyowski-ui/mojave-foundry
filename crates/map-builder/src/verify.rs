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
use vault_formats::pro::{BasicFlags, Category, PrototypeData, PrototypeKind, PrototypeLookup};
use vault_formats::{
    ArtCatalog, ByteSource, Fid, FrmArchive, GameProfile, LayeredVfs, LocalPrototypeCatalog,
    LstKind, LstTable, MAX_ASSET_BYTES, MemoryDir, MsgCatalog, Prototype, Vfs, VfsPath,
};

use crate::BuildError;
use crate::assemble::{EXIT_GRID_PID, MAPS_TXT};
use crate::islands;

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
    /// Art lists of the layer that do not start from the whole base list
    /// (standalone walls and tiles), by art directory in category order.
    pub replaced_art_lists: Vec<String>,
    /// Prototype art identifiers of the layer's prototypes resolved to a
    /// file.
    pub prototype_fids: usize,
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
///    (`LstKind::for_path`); an art list holds at most 4,096 entries. An
///    art list whose base list is supplied either starts with every base
///    entry (same name and alias at each position), or is a standalone
///    walls or tiles list that starts with the base entries
///    `emit::standalone_reserved` names (it is then listed in
///    `replaced_art_lists`); any other art list that drops base entries is
///    refused.
/// 4. Every `.msg`: `MsgCatalog::parse_strict`.
/// 5. Every `.pro`: `Prototype::parse_for` (Fallout 2) round-trips to the
///    same bytes, and `LocalPrototypeCatalog` (Fallout 2, over the mounted
///    layer) resolves its PID to it through its list position; its art FID
///    resolves as the map objects' do (in 6).
/// 6. Every `.map`: `MapFile::parse_for` (Fallout 2) through that catalog,
///    version 20, no diagnostics; the header name is the file name in
///    capitals and the header number its registered number; every object
///    art FID, every floor tile (the blank 1 included, since a floor of 1
///    is drawn) and every roof tile other than 1 (never drawn) resolves
///    through `ArtCatalog::load` (the mounted art lists) to a list entry;
///    an entry past the base entries the mounted list kept at their
///    positions (all of the base list, or the reserved entries of a
///    standalone one) must name a file the layer holds; every exit grid
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

    // Per art list, how many leading entries are the install's art: the
    // base entries the mounted list kept at their positions.
    let mut base_len: BTreeMap<u8, usize> = BTreeMap::new();
    for (category, directory) in ART_DIRECTORIES.iter().enumerate() {
        let path = format!("art/{directory}/{directory}.lst");
        let Some(bytes) = base_files.get(&path) else {
            continue;
        };
        let base =
            LstTable::parse(bytes, LstKind::Art).map_err(|e| BuildError::verify(&path, e))?;
        let Some(ours) = layer.files().get(&path) else {
            base_len.insert(category as u8, base.entries().len());
            continue;
        };
        let ours = LstTable::parse(ours, LstKind::Art).map_err(|e| BuildError::verify(&path, e))?;
        let kept = ours
            .entries()
            .iter()
            .zip(base.entries())
            .take_while(|(a, b)| a.name == b.name && a.alias == b.alias)
            .count();
        if kept < base.entries().len() {
            let Some(reserved) = emit::standalone_reserved(category as u8) else {
                return Err(BuildError::verify(
                    &path,
                    format!(
                        "keeps {kept} of the base list's {} entries; only a standalone walls or \
                         tiles list may drop base entries",
                        base.entries().len()
                    ),
                ));
            };
            let needed = reserved.min(base.entries().len());
            if kept < needed {
                return Err(BuildError::verify(
                    &path,
                    format!("a standalone list must keep the base list's first {needed} entries"),
                ));
            }
            report.replaced_art_lists.push((*directory).to_owned());
        }
        base_len.insert(category as u8, kept);
    }

    // 2-5, 7, 8: file by file.
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let mut map_paths = Vec::new();
    let mut prototype_fids: Vec<(String, Fid)> = Vec::new();
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
                prototype_fids.push((path.clone(), Fid(prototype.art_id)));
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
    for (path, fid) in &prototype_fids {
        resolve(*fid, path)?;
        report.prototype_fids += 1;
    }
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
                // Roof index 1 is never drawn; floor index 1 is.
                let roof = (tile.roof() != 1).then_some(tile.roof());
                for index in std::iter::once(tile.floor()).chain(roof) {
                    if seen.insert(index) {
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

/// What [`verify_links`] checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkCheck {
    /// Exit grids checked, over every map and elevation.
    pub grids: usize,
    /// Distinct (map, elevation) to (map, elevation) links.
    pub links: usize,
    /// Of those, the ones with no exit grid leading back, as
    /// `"<file> elevation <e> -> <file> elevation <f>"`.
    pub one_way: Vec<String>,
}

/// Every map of `layer` parsed back through `vault-formats` over
/// `base_files`: the registered file names by number, and the maps by file.
#[allow(clippy::type_complexity)]
fn parse_maps(
    layer: &emit::ModLayer,
    base_files: &BTreeMap<String, Vec<u8>>,
) -> Result<(BTreeMap<u32, String>, BTreeMap<String, MapFile>), BuildError> {
    let text = layer
        .manifest_text()
        .map_err(|e| BuildError::verify(MANIFEST, e))?;
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
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let mut by_number: BTreeMap<u32, String> = BTreeMap::new();
    for map in &layer.manifest().maps {
        by_number.insert(map.number, map.file.clone());
    }
    let mut maps: BTreeMap<String, MapFile> = BTreeMap::new();
    for file in by_number.values() {
        let path = format!("maps/{file}");
        let bytes = layer
            .files()
            .get(&path)
            .ok_or_else(|| BuildError::verify(&path, "is registered but missing"))?;
        let map = MapFile::parse_for(bytes, &catalog, GameProfile::Fallout2)
            .map_err(|e| BuildError::verify(&path, e))?;
        maps.insert(file.clone(), map);
    }
    Ok((by_number, maps))
}

/// What [`verify_clearance`] checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClearanceCheck {
    /// Keep-clear hexes over every map and elevation.
    pub hexes: usize,
    /// Items, containers and critters checked against them (those on an
    /// elevation with links).
    pub objects: usize,
}

/// The keep-clear rule on the written maps ([`crate::clearance`]): on every
/// elevation, no ground item, container or critter stands on an exit grid
/// or arrival hex of `maps`' links, nor (items and containers that do not
/// block their hex, and critters) within
/// [`crate::clearance::STAIRS_KEEP_CLEAR_RADIUS`] steps of a stair's grid.
/// A failed check is [`BuildError::Verify`] naming the map, elevation, hex
/// and object.
pub fn verify_clearance(
    layer: &emit::ModLayer,
    base_files: &BTreeMap<String, Vec<u8>>,
    maps: &[crate::BuiltMap],
) -> Result<ClearanceCheck, BuildError> {
    // Per (file, elevation): the exact keep-clear set, and the set that
    // blocking containers are exempt from (the stairs' surroundings).
    let mut exact: BTreeMap<(String, u8), BTreeSet<i32>> = BTreeMap::new();
    let mut around: BTreeMap<(String, u8), BTreeSet<i32>> = BTreeMap::new();
    for map in maps {
        for link in &map.links {
            let key = (map.file.clone(), link.elevation);
            exact
                .entry(key.clone())
                .or_default()
                .extend(link.grid_hexes.iter().copied());
            exact
                .entry((link.to_file.clone(), link.to_elevation))
                .or_default()
                .insert(link.to_hex);
            let ring = crate::clearance::keep_clear([(link.kind, link.grid_hexes.as_slice())], []);
            around.entry(key).or_default().extend(ring);
        }
    }
    if exact.is_empty() {
        return Ok(ClearanceCheck::default());
    }
    let (_, parsed) = parse_maps(layer, base_files)?;
    let mut check = ClearanceCheck {
        hexes: around.values().map(BTreeSet::len).sum(),
        ..ClearanceCheck::default()
    };
    for (key, hexes) in &exact {
        if !around.contains_key(key) {
            check.hexes += hexes.len();
        }
    }
    for (file, map) in &parsed {
        let path = format!("maps/{file}");
        for (e, objects) in map.objects.elevations.iter().enumerate() {
            let key = (path.clone(), e as u8);
            let Some(exact) = exact.get(&key) else {
                continue;
            };
            let around = around.get(&key);
            for object in objects {
                let category = Fid(object.art_id).category();
                if !matches!(category, 0 | 1) {
                    continue;
                }
                let Some(hex) = object.hex else { continue };
                check.objects += 1;
                let blocks = category == 0 && object.flags.0 & BasicFlags::NONBLOCKING.0 == 0;
                let bad = exact.contains(&hex)
                    || (!blocks && around.is_some_and(|set| set.contains(&hex)));
                if bad {
                    return Err(BuildError::verify(
                        &path,
                        format!(
                            "{} {} stands on hex {hex} of elevation {e}, on or beside an exit grid or arrival of a link",
                            if category == 0 {
                                "item or container"
                            } else {
                                "critter"
                            },
                            object.id
                        ),
                    ));
                }
            }
        }
    }
    Ok(check)
}

/// One elevation's exit grids: hex -> (map, hex, elevation).
type GridsByHex = BTreeMap<i32, (i32, i32, i32)>;

/// The label of a link between two map elevations.
pub(crate) fn pair_label(from: &str, from_elevation: u8, to: &str, to_elevation: u8) -> String {
    format!("{from} elevation {from_elevation} -> {to} elevation {to_elevation}")
}

/// Every exit grid of every map of `layer` (mounted over `base_files`,
/// parsed through `vault-formats`) followed to its destination:
///
/// - the destination map is a map of the layer (a real map or a stub);
/// - the destination elevation exists there (it has tiles or objects);
/// - the destination hex is 1 to 39999 (0 would keep the map's entrance),
///   holds no exit grid of that elevation (arriving never leaves again)
///   and no object that stops movement (the engine's rule, critters
///   included): the hex is walkable there;
/// - every link (map and elevation to map and elevation) is counted, and
///   one with no grid leading back is listed in [`LinkCheck::one_way`].
///
/// A failed check is [`BuildError::Verify`] naming the map and grid.
pub fn verify_links(
    layer: &emit::ModLayer,
    base_files: &BTreeMap<String, Vec<u8>>,
) -> Result<LinkCheck, BuildError> {
    let (by_number, maps) = parse_maps(layer, base_files)?;
    let number_of: BTreeMap<&str, u32> = by_number
        .iter()
        .map(|(number, file)| (file.as_str(), *number))
        .collect();
    // Per (file, elevation): exit grids by hex, and blocked hexes.
    let mut grids: BTreeMap<(String, u8), GridsByHex> = BTreeMap::new();
    let mut blocked: BTreeMap<(String, u8), Vec<bool>> = BTreeMap::new();
    for (file, map) in &maps {
        for (e, objects) in map.objects.elevations.iter().enumerate() {
            let key = (file.clone(), e as u8);
            blocked.insert(key.clone(), islands::engine_blocked(objects));
            let mut here = BTreeMap::new();
            for object in objects {
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
                    && let Some(at) = object.hex
                {
                    here.insert(at, (*to, *hex, *elevation));
                }
            }
            grids.insert(key, here);
        }
    }
    let mut check = LinkCheck::default();
    let mut pairs: BTreeSet<(String, u8, String, u8)> = BTreeSet::new();
    for ((file, e), here) in &grids {
        for (at, (to, hex, elevation)) in here {
            let place = format!("maps/{file}");
            let what = format!("the exit grid on hex {at}, elevation {e}");
            let to_file = u32::try_from(*to)
                .ok()
                .and_then(|n| by_number.get(&n))
                .ok_or_else(|| {
                    BuildError::verify(
                        &place,
                        format!("{what} leads to map {to}, not in the layer"),
                    )
                })?;
            let to_elevation = u8::try_from(*elevation)
                .ok()
                .filter(|e| *e <= 2)
                .ok_or_else(|| {
                    BuildError::verify(&place, format!("{what} leads to elevation {elevation}"))
                })?;
            let target = &maps[to_file];
            let level = usize::from(to_elevation);
            if target.tiles[level].is_none() && target.objects.elevations[level].is_empty() {
                return Err(BuildError::verify(
                    &place,
                    format!("{what} leads to {to_file} elevation {to_elevation}, which is omitted"),
                ));
            }
            if !(1..40_000).contains(hex) {
                return Err(BuildError::verify(
                    &place,
                    format!("{what} leads to hex {hex}; 0 would keep the entrance"),
                ));
            }
            let key = (to_file.clone(), to_elevation);
            if grids[&key].contains_key(hex) {
                return Err(BuildError::verify(
                    &place,
                    format!("{what} arrives on an exit grid of {to_file} (hex {hex})"),
                ));
            }
            if blocked[&key][*hex as usize] {
                return Err(BuildError::verify(
                    &place,
                    format!("{what} arrives on hex {hex} of {to_file}, which an object blocks"),
                ));
            }
            check.grids += 1;
            pairs.insert((file.clone(), *e, to_file.clone(), to_elevation));
        }
    }
    check.links = pairs.len();
    for (file, e, to, f) in &pairs {
        let from_number = number_of[file.as_str()] as i32;
        let back = grids[&(to.clone(), *f)]
            .values()
            .any(|(map, _, elevation)| *map == from_number && *elevation == i32::from(*e));
        if !back {
            check.one_way.push(pair_label(file, *e, to, *f));
        }
    }
    Ok(check)
}

/// What [`verify_loot`] checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LootCheck {
    /// Item prototypes of the layer checked.
    pub item_prototypes: usize,
    /// Weapons with a default ammunition PID: it resolves to ammunition of
    /// the weapon's calibre.
    pub weapons_with_ammo: usize,
    /// Miscellaneous items with a power source that resolves to
    /// ammunition of its calibre.
    pub powered: usize,
    /// Item prototypes (containers excepted) whose inventory art resolves
    /// to an `art/inven` FRM.
    pub inventory_art: usize,
    /// Item prototypes (containers excepted) without inventory art, by
    /// PID: generic converted items have none.
    pub no_inventory_art: Vec<u32>,
    /// Prototypes whose name and description records are in their mounted
    /// message file.
    pub messages: usize,
    /// Inventory records on the maps (every depth) whose PID resolves to
    /// an item and whose art resolves.
    pub inventory_records: usize,
    /// Maps that parse and write back to the same bytes.
    pub maps: usize,
}

/// Mount `layer` (and its manifest text) over `base_files`.
fn mount(
    layer: &emit::ModLayer,
    base_files: &BTreeMap<String, Vec<u8>>,
) -> Result<LayeredVfs, BuildError> {
    let text = layer
        .manifest_text()
        .map_err(|e| BuildError::verify(MANIFEST, e))?;
    let base = tree(base_files.clone(), "base files")?;
    let mut layer_files: Vec<(String, Vec<u8>)> = layer
        .files()
        .iter()
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    layer_files.push((MANIFEST.to_owned(), text.into_bytes()));
    let mounted = tree(layer_files, "layer files")?;
    Ok(LayeredVfs::new(
        Arc::new(base),
        vec![vault_formats::ModLayer::from_tree("map-builder", mounted)],
    ))
}

/// The items, inventories and names of `layer` (mounted over
/// `base_files`), parsed back through `vault-formats`:
///
/// - every `.pro` of the layer with a message base has its name and
///   description records (base and base + 1) in its category's mounted
///   message file;
/// - every item prototype of the layer but a container either has
///   inventory art that resolves (`ArtCatalog`, category 7) to an FRM that
///   parses, or is listed in [`LootCheck::no_inventory_art`];
/// - a weapon's default ammunition PID, and a miscellaneous item's power
///   source, resolves to ammunition of the same calibre (a layer
///   prototype; a base PID must resolve as ammunition);
/// - every map parses (`MapFile::parse_for`, Fallout 2) and writes back to
///   the same bytes; every inventory record, at every depth, has an item
///   PID that resolves, a quantity of 1 or more, and art that resolves.
///
/// [`crate::build_layer`] runs it on a layer built with a catalogue and
/// refuses one whose catalogue items lack inventory art.
pub fn verify_loot(
    layer: &emit::ModLayer,
    base_files: &BTreeMap<String, Vec<u8>>,
) -> Result<LootCheck, BuildError> {
    use vault_formats::pro::{ItemData, ItemSubtype};
    let vfs = mount(layer, base_files)?;
    let catalog = LocalPrototypeCatalog::for_game(&vfs, GameProfile::Fallout2);
    let art = ArtCatalog::load(&vfs).map_err(|e| BuildError::verify("art lists", e))?;
    let mut check = LootCheck::default();
    let mut messages: BTreeMap<Category, MsgCatalog> = BTreeMap::new();
    let mut items: BTreeMap<u32, Prototype> = BTreeMap::new();
    let mut frm_checked: BTreeSet<String> = BTreeSet::new();
    let mut frm_at = |fid: Fid, at: &str| -> Result<(), BuildError> {
        let target = art
            .path(fid)
            .map_err(|e| BuildError::verify(at, format!("FID {:#010x}: {e}", fid.0)))?;
        if frm_checked.insert(target.clone()) {
            let bytes = vfs.read(&target).map_err(|e| {
                BuildError::verify(at, format!("FID {:#010x} names {target}: {e}", fid.0))
            })?;
            FrmArchive::parse(&bytes).map_err(|e| BuildError::verify(&target, e))?;
        }
        Ok(())
    };
    for (path, bytes) in layer.files() {
        if !path.ends_with(".pro") {
            continue;
        }
        let prototype = Prototype::parse_for(bytes, GameProfile::Fallout2)
            .map_err(|e| BuildError::verify(path, e))?;
        if prototype.message_base >= 0 {
            let category = prototype.category;
            if let std::collections::btree_map::Entry::Vacant(slot) = messages.entry(category) {
                let file = emit::proto::message_file_path(category, crate::LANGUAGE);
                let bytes = vfs
                    .read(&file)
                    .map_err(|e| BuildError::verify(path, format!("{file}: {e}")))?;
                slot.insert(
                    MsgCatalog::parse_strict(&bytes).map_err(|e| BuildError::verify(&file, e))?,
                );
            }
            let file = &messages[&category];
            for id in [prototype.message_base, prototype.message_base + 1] {
                if file.get(id).is_none() {
                    return Err(BuildError::verify(
                        path,
                        format!(
                            "message {id} is not in {}",
                            emit::proto::message_file_path(category, crate::LANGUAGE)
                        ),
                    ));
                }
            }
            check.messages += 1;
        }
        if let PrototypeData::Item(item) = &prototype.data {
            check.item_prototypes += 1;
            if !matches!(item.data, ItemData::Container { .. }) {
                if item.inventory_art == u32::MAX {
                    check.no_inventory_art.push(prototype.pid);
                } else {
                    let fid = Fid(item.inventory_art);
                    if fid.category() != emit::icon::INVENTORY_CATEGORY {
                        return Err(BuildError::verify(
                            path,
                            format!("inventory art {:#010x} is not art/inven art", fid.0),
                        ));
                    }
                    frm_at(fid, path)?;
                    check.inventory_art += 1;
                }
            }
            items.insert(prototype.pid, prototype);
        }
    }
    // Ammunition links, once every layer item is known.
    let caliber_of = |pid: i32, at: &str| -> Result<Option<i32>, BuildError> {
        let pid = pid as u32;
        if let Some(prototype) = items.get(&pid) {
            return match &prototype.data {
                PrototypeData::Item(item) => match item.data {
                    ItemData::Ammunition { caliber, .. } => Ok(Some(caliber)),
                    _ => Err(BuildError::verify(
                        at,
                        format!("{pid:#010x} is not ammunition"),
                    )),
                },
                _ => Err(BuildError::verify(
                    at,
                    format!("{pid:#010x} is not an item"),
                )),
            };
        }
        let info = catalog
            .lookup(pid)
            .map_err(|e| BuildError::verify(at, format!("ammunition {pid:#010x}: {e}")))?;
        if info.kind != PrototypeKind::Item(ItemSubtype::Ammunition) {
            return Err(BuildError::verify(
                at,
                format!("{pid:#010x} is not ammunition"),
            ));
        }
        Ok(None)
    };
    for (pid, prototype) in &items {
        let at = format!("item prototype {pid:#010x}");
        let PrototypeData::Item(item) = &prototype.data else {
            continue;
        };
        match &item.data {
            ItemData::Weapon(weapon) if weapon.ammo_pid != -1 => {
                if let Some(caliber) = caliber_of(weapon.ammo_pid, &at)?
                    && caliber != weapon.caliber
                {
                    return Err(BuildError::verify(
                        &at,
                        format!(
                            "weapon of calibre {} loads ammunition {:#010x} of calibre {caliber}",
                            weapon.caliber, weapon.ammo_pid
                        ),
                    ));
                }
                check.weapons_with_ammo += 1;
            }
            ItemData::Misc {
                power_pid,
                power_type,
                ..
            } if *power_pid != -1 => {
                if let Some(caliber) = caliber_of(*power_pid, &at)?
                    && caliber != *power_type
                {
                    return Err(BuildError::verify(
                        &at,
                        format!(
                            "power source {power_pid:#010x} is of calibre {caliber}, not \
                             {power_type}"
                        ),
                    ));
                }
                check.powered += 1;
            }
            _ => {}
        }
    }
    // Maps: inventories at every depth, and the round trip.
    fn walk(
        object: &vault_formats::map::MapObject,
        catalog: &LocalPrototypeCatalog<'_>,
        path: &str,
        frm_at: &mut dyn FnMut(Fid, &str) -> Result<(), BuildError>,
        records: &mut usize,
    ) -> Result<(), BuildError> {
        for entry in &object.inventory {
            let child = &entry.object;
            let at = format!("object {} holds object {}", object.id, child.id);
            if entry.quantity == 0 {
                return Err(BuildError::verify(path, format!("{at}: quantity 0")));
            }
            let info = catalog
                .lookup(child.pid)
                .map_err(|e| BuildError::verify(path, format!("{at}: {e}")))?;
            if !matches!(info.kind, PrototypeKind::Item(_)) {
                return Err(BuildError::verify(
                    path,
                    format!("{at}: PID {:#010x} is not an item", child.pid),
                ));
            }
            frm_at(Fid(child.art_id), path)?;
            *records += 1;
            walk(child, catalog, path, frm_at, records)?;
        }
        Ok(())
    }
    for (path, bytes) in layer.files() {
        if !path.ends_with(".map") {
            continue;
        }
        let map = MapFile::parse_for(bytes, &catalog, GameProfile::Fallout2)
            .map_err(|e| BuildError::verify(path, e))?;
        for object in map.objects.elevations.iter().flatten() {
            walk(
                object,
                &catalog,
                path,
                &mut frm_at,
                &mut check.inventory_records,
            )?;
        }
        let written = map.write().map_err(|e| BuildError::verify(path, e))?;
        if written != *bytes {
            return Err(BuildError::verify(
                path,
                "does not write back to the same bytes",
            ));
        }
        check.maps += 1;
    }
    Ok(check)
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
