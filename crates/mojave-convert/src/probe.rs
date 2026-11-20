//! Real-data probes. They read a user's own install, print only structural
//! tallies (counts, formats, error messages with offsets), and write nothing.
//! They exist to calibrate the parsers against files the synthetic fixtures
//! cannot cover.

use std::collections::BTreeMap;
use std::path::Path;

use fnv_bsa::BsaArchive;

/// Opens an archive, verifies its name hashes, and reads every entry (or
/// every `step`-th one). With `decode_dds`, also decodes every `.dds`.
pub fn bsa(path: &Path, step: usize, decode_dds: bool) -> Result<(), String> {
    let archive = BsaArchive::open(path).map_err(|e| format!("open: {e}"))?;
    let entries = archive.entries();
    println!("archive: {}", path.display());
    println!(
        "  entries: {}  archive flags: {:#x}  file flags: {:#x}  embedded names: {}",
        entries.len(),
        archive.archive_flags(),
        archive.file_flags(),
        archive.embeds_names()
    );
    let mismatches = archive.verify_hashes();
    println!("  hash mismatches: {}", mismatches.len());
    for mismatch in mismatches.iter().take(5) {
        println!("    {mismatch:?}");
    }

    let mut by_extension: BTreeMap<String, usize> = BTreeMap::new();
    let mut compressed = 0usize;
    for entry in entries {
        let extension = entry
            .path
            .rsplit_once('.')
            .map_or("", |(_, ext)| ext)
            .to_owned();
        *by_extension.entry(extension).or_default() += 1;
        compressed += usize::from(entry.compressed);
    }
    println!("  compressed entries: {compressed}");
    println!("  by extension: {by_extension:?}");

    let mut read_ok = 0usize;
    let mut read_errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut bytes = 0u64;
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut dds_errors: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut dds_ok = 0usize;
    for entry in entries.iter().step_by(step.max(1)) {
        match archive.read(entry) {
            Ok(data) => {
                read_ok += 1;
                bytes += data.len() as u64;
                if decode_dds && entry.path.ends_with(".dds") {
                    match dds::info(&data) {
                        Ok(info) => {
                            let key = format!(
                                "{:?}{}",
                                info.format,
                                if info.cubemap { " cube" } else { "" }
                            );
                            *formats.entry(key).or_default() += 1;
                        }
                        Err(error) => {
                            let slot = dds_errors
                                .entry(format!("info: {error}"))
                                .or_insert((0, entry.path.clone()));
                            slot.0 += 1;
                            continue;
                        }
                    }
                    match dds::decode(&data) {
                        Ok(image) => {
                            if image.rgba.len() as u64
                                == u64::from(image.width) * u64::from(image.height) * 4
                            {
                                dds_ok += 1;
                            } else {
                                let slot = dds_errors
                                    .entry("decode: buffer length mismatch".to_owned())
                                    .or_insert((0, entry.path.clone()));
                                slot.0 += 1;
                            }
                        }
                        Err(error) => {
                            let slot = dds_errors
                                .entry(format!("decode: {error}"))
                                .or_insert((0, entry.path.clone()));
                            slot.0 += 1;
                        }
                    }
                }
            }
            Err(error) => {
                // Offsets differ per entry; group by the message's first clause.
                let text = error.to_string();
                let key = text.split(':').next().unwrap_or(&text).to_owned();
                *read_errors.entry(key).or_default() += 1;
            }
        }
    }
    println!("  read ok: {read_ok} ({bytes} bytes)  read errors: {read_errors:?}");
    if decode_dds {
        println!("  dds decoded ok: {dds_ok}");
        println!("  dds formats: {formats:#?}");
        for (message, (count, first)) in &dds_errors {
            println!("  dds error x{count}: {message}  (first: {first})");
        }
    }
    Ok(())
}

/// Replaces digit runs so that per-file offsets do not split a tally.
fn normalize(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut in_digits = false;
    for c in message.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

/// Parses every `.nif` in an archive (or every `step`-th one) whose path
/// starts with `prefix`, extracts the static mesh, and round-trips a sample
/// through the glb writer and reader.
pub fn nif(path: &Path, step: usize, prefix: &str) -> Result<(), String> {
    let archive = BsaArchive::open(path).map_err(|e| format!("open: {e}"))?;
    let mut total = 0usize;
    let mut parsed = 0usize;
    let mut extracted = 0usize;
    let mut empty = 0usize;
    let mut primitives = 0usize;
    let mut vertices = 0u64;
    let mut triangles = 0u64;
    let mut glb_checked = 0usize;
    let mut glb_bad = 0usize;
    let mut errors: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut skipped: BTreeMap<String, u64> = BTreeMap::new();
    let mut alpha: BTreeMap<String, usize> = BTreeMap::new();
    let mut with_collision = 0usize;
    let mut untextured = 0usize;
    let note = |errors: &mut BTreeMap<String, (usize, String)>, key: String, file: &str| {
        let slot = errors.entry(key).or_insert((0, file.to_owned()));
        slot.0 += 1;
    };
    for entry in archive
        .entries()
        .iter()
        .filter(|e| e.path.ends_with(".nif") && e.path.starts_with(prefix))
        .step_by(step.max(1))
    {
        total += 1;
        let data = match archive.read(entry) {
            Ok(data) => data,
            Err(error) => {
                note(
                    &mut errors,
                    format!("read: {}", normalize(&error.to_string())),
                    &entry.path,
                );
                continue;
            }
        };
        let file = match nif::parse(&data) {
            Ok(file) => file,
            Err(error) => {
                note(
                    &mut errors,
                    format!("parse: {}", normalize(&error.to_string())),
                    &entry.path,
                );
                continue;
            }
        };
        parsed += 1;
        for (kind, count) in file.skipped_blocks() {
            *skipped.entry(kind.clone()).or_default() += u64::from(count);
        }
        let mesh = match nif::extract_mesh(&file) {
            Ok(mesh) => mesh,
            Err(error) => {
                note(
                    &mut errors,
                    format!("extract: {}", normalize(&error.to_string())),
                    &entry.path,
                );
                continue;
            }
        };
        extracted += 1;
        if mesh.primitives.is_empty() {
            empty += 1;
        }
        with_collision += usize::from(mesh.collision.is_some());
        for primitive in &mesh.primitives {
            primitives += 1;
            vertices += primitive.positions.len() as u64;
            triangles += (primitive.indices.len() / 3) as u64;
            let key = match primitive.material.alpha {
                nif::AlphaMode::Opaque => "opaque",
                nif::AlphaMode::Mask { .. } => "mask",
                nif::AlphaMode::Blend => "blend",
            };
            *alpha.entry(key.to_owned()).or_default() += 1;
            if primitive.material.textures.iter().all(String::is_empty) {
                untextured += 1;
            }
        }
        if total.is_multiple_of(25) {
            glb_checked += 1;
            let bytes = nif::glb::write(&mesh);
            match nif::glb::read(&bytes) {
                Ok(back) if back == mesh => {}
                Ok(_) => glb_bad += 1,
                Err(error) => {
                    glb_bad += 1;
                    note(
                        &mut errors,
                        format!("glb: {}", normalize(&error.to_string())),
                        &entry.path,
                    );
                }
            }
        }
    }
    println!("nif files: {total}  parsed: {parsed}  extracted: {extracted}  empty meshes: {empty}");
    println!("primitives: {primitives}  vertices: {vertices}  triangles: {triangles}");
    println!(
        "alpha modes: {alpha:?}  untextured primitives: {untextured}  with collision: {with_collision}"
    );
    println!("glb round trips: {glb_checked} checked, {glb_bad} differ");
    let mut ranked: Vec<_> = errors.iter().collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.1.0));
    println!("error classes: {}", ranked.len());
    for (message, (count, first)) in ranked.iter().take(40) {
        println!("  x{count}: {message}  (first: {first})");
    }
    let mut ranked: Vec<_> = skipped.iter().collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(*entry.1));
    println!("skipped block types: {}", ranked.len());
    for (kind, count) in ranked.iter().take(60) {
        println!("  {count:>8}  {kind}");
    }
    Ok(())
}

/// Walks a plugin, decodes every record through the typed table, and
/// tallies what did not decode cleanly. `needle` selects cells by editor id
/// substring (case-insensitive) for a closer look.
pub fn esm(path: &Path, needle: &str) -> Result<(), String> {
    use fnv_esm::{Plugin, Record};
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let plugin = Plugin::parse(&bytes).map_err(|e| format!("parse: {e}"))?;
    let header = plugin.header();
    println!(
        "plugin: version {}  declared records {}  masters {:?}",
        header.version, header.record_count, header.masters
    );
    let needle = needle.to_ascii_lowercase();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut errors: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut unknown: BTreeMap<String, (usize, std::collections::BTreeSet<usize>)> = BTreeMap::new();
    let mut worlds: Vec<String> = Vec::new();
    let mut cells_of_interest: BTreeMap<u32, String> = BTreeMap::new();
    let mut per_cell: BTreeMap<u32, BTreeMap<String, usize>> = BTreeMap::new();
    let mut land_range: BTreeMap<u32, (f32, f32)> = BTreeMap::new();
    let mut exterior_cells = 0usize;
    let mut interior_cells = 0usize;
    let mut script_sources = 0usize;
    let mut scripts = 0usize;
    let mut walker = plugin.walk();
    loop {
        let record = match walker.next_record() {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(error) => {
                println!("walker stopped: {error}");
                break;
            }
        };
        let sig = record.sig().to_string();
        *counts.entry(sig.clone()).or_default() += 1;
        let cell = walker.context().cell().map(|id| id.0);
        let decoded = match record.decode() {
            Ok(decoded) => decoded,
            Err(error) => {
                let slot = errors
                    .entry(format!("{sig}: {}", normalize(&error.to_string())))
                    .or_insert((0, record.offset));
                slot.0 += 1;
                continue;
            }
        };
        for sub in decoded.unknown_subrecords() {
            let slot = unknown.entry(format!("{sig}/{}", sub.sig)).or_default();
            slot.0 += 1;
            if slot.1.len() < 6 {
                slot.1.insert(sub.data.len());
            }
        }
        match &decoded {
            Record::World(world) => {
                worlds.push(world.common.editor_id.clone().unwrap_or_default());
            }
            Record::Cell(c) => {
                if c.grid.is_some() {
                    exterior_cells += 1;
                } else {
                    interior_cells += 1;
                }
                let id = c.common.editor_id.clone().unwrap_or_default();
                if !needle.is_empty() && id.to_ascii_lowercase().contains(&needle) {
                    let grid = c.grid.as_ref().map(|g| (g.x, g.y));
                    cells_of_interest.insert(decoded.form_id().0, format!("{id} grid {grid:?}"));
                }
            }
            Record::Land(land) => {
                if let (Some(cell), Some(heights)) = (cell, land.heights.as_ref()) {
                    let mut lo = f32::INFINITY;
                    let mut hi = f32::NEG_INFINITY;
                    for h in heights.iter() {
                        lo = lo.min(*h);
                        hi = hi.max(*h);
                    }
                    land_range.insert(cell, (lo, hi));
                }
            }
            Record::Script(script) => {
                scripts += 1;
                script_sources += usize::from(script.body.source.is_some());
            }
            _ => {}
        }
        if let Some(cell) = cell
            && sig != "CELL"
        {
            *per_cell.entry(cell).or_default().entry(sig).or_default() += 1;
        }
    }
    let total: usize = counts.values().sum();
    println!(
        "records walked: {total}  distinct signatures: {}",
        counts.len()
    );
    let typed: Vec<String> = fnv_esm::DECODERS
        .iter()
        .map(|d| d.sig.to_string())
        .collect();
    for sig in &typed {
        println!("  {sig}: {}", counts.get(sig).copied().unwrap_or(0));
    }
    println!("cells: {exterior_cells} exterior, {interior_cells} interior");
    println!("scripts: {scripts}, with source text: {script_sources}");
    println!("worldspaces: {worlds:?}");
    println!("decode error classes: {}", errors.len());
    for (message, (count, offset)) in &errors {
        println!("  x{count}: {message} (first at {offset:#x})");
    }
    println!("subrecords left undecoded in typed records (record/subrecord: count, sizes seen):");
    let mut ranked: Vec<_> = unknown.iter().collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.1.0));
    for (key, (count, sizes)) in ranked.iter().take(70) {
        println!("  {key}: {count} {sizes:?}");
    }
    println!("cells matching {needle:?}: {}", cells_of_interest.len());
    for (form, label) in &cells_of_interest {
        println!(
            "  {form:08x} {label}  children {:?}  land z range {:?}",
            per_cell.get(form).cloned().unwrap_or_default(),
            land_range.get(form)
        );
    }
    Ok(())
}

/// Opens a Fallout 2 install's main archive through `vault-formats` and
/// reads the palette, to confirm the `--f2` inputs resolve.
pub fn f2(dir: &Path) -> Result<(), String> {
    use vault_formats::{Dat2Archive, Palette, Vfs};
    let master = ["master.dat", "MASTER.DAT"]
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| format!("no master.dat in {}", dir.display()))?;
    let archive = Dat2Archive::open(&master).map_err(|e| format!("open: {e}"))?;
    println!(
        "archive: {}  entries: {}",
        master.display(),
        archive.entries().len()
    );
    let bytes = archive
        .read("color.pal")
        .map_err(|e| format!("color.pal: {e}"))?;
    let palette = Palette::parse(&bytes).map_err(|e| format!("palette: {e}"))?;
    let mapped = (0..=255u8).filter(|i| palette.is_mapped(*i)).count();
    println!("color.pal: {} bytes, {mapped} mapped indices", bytes.len());
    Ok(())
}

/// Reads one map of a written mod layer back from disk and reports where a
/// walker can go, by the engine's movement rule: an object stops movement
/// when its art is critter, scenery or wall art and it is neither hidden
/// nor non-blocking; a multi-hex object also stops its six neighbours; and
/// no step leaves an outer-border hex. Prints, for each exit grid, whether
/// it can be reached from the entry hex and a chain of waypoints `hop`
/// steps apart, short enough for the engine's capped route search. For each
/// hex in `from` (the points other maps' exits arrive at) it then reports
/// what can be reached without stepping on an exit grid.
pub fn walk(layer_dir: &Path, map: &str, hop: usize, from: &[i32]) -> Result<(), String> {
    use std::collections::VecDeque;
    use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
    use vault_formats::pro::BasicFlags;
    use vault_formats::pro_catalog::LocalPrototypeCatalog;
    use vault_formats::vfs::{ByteSource, MemoryDir, MemoryMount};
    use vault_formats::{Fid, GameProfile, HexGrid};

    const HEXES: usize = 40_000;
    let layer = map_builder::read_layer(layer_dir).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for (path, bytes) in layer.files() {
        if path == "vault-mod.toml" || path == "README.txt" {
            continue;
        }
        let source = ByteSource::new(bytes.clone()).map_err(|e| format!("{path}: {e}"))?;
        entries.push((path.clone(), source));
    }
    let tree = MemoryDir::new(entries).map_err(|e| e.to_string())?;
    let provider = MemoryMount {
        profile: GameProfile::Fallout2,
        loose: Some(tree),
        ..MemoryMount::default()
    }
    .mount()
    .map_err(|e| e.to_string())?;
    let catalog = LocalPrototypeCatalog::for_game(&provider, GameProfile::Fallout2);
    let path = format!("maps/{map}");
    let bytes = layer
        .files()
        .get(&path)
        .ok_or_else(|| format!("{path} is not in the layer"))?;
    let parsed = MapFile::parse_for(bytes, &catalog, GameProfile::Fallout2)
        .map_err(|e| format!("{path}: {e}"))?;

    let neighbours =
        |hex: i32| (0..6u8).filter_map(move |o| HexGrid::neighbor(hex, o).ok().flatten());
    let border = |hex: i32| {
        HexGrid::coordinates(hex)
            .map(|[c, r]| c == 0 || r == 0 || c == 199 || r == 199)
            .unwrap_or(true)
    };
    let mut blocked = vec![false; HEXES];
    let mut exits = Vec::new();
    let mut blockers = 0usize;
    for object in &parsed.objects.elevations[0] {
        let Some(hex) = object.hex.filter(|h| (0..HEXES as i32).contains(h)) else {
            continue;
        };
        if let ObjectState::Other {
            detail: ObjectDetail::ExitGrid {
                map: to, hex: at, ..
            },
            ..
        } = &object.state
        {
            exits.push((hex, *to, *at));
        }
        let stops = object.flags.0 & (BasicFlags::HIDDEN.0 | BasicFlags::NONBLOCKING.0) == 0
            && matches!(Fid(object.art_id).category(), 1..=3);
        if !stops {
            continue;
        }
        blockers += 1;
        blocked[hex as usize] = true;
        if object.flags.contains(BasicFlags::MULTIHEX) {
            for next in neighbours(hex) {
                blocked[next as usize] = true;
            }
        }
    }

    let entry = parsed.header.entry_hex;
    let mut parent = vec![-2i32; HEXES];
    let mut queue = VecDeque::new();
    if (0..HEXES as i32).contains(&entry) && !blocked[entry as usize] {
        parent[entry as usize] = -1;
        queue.push_back(entry);
    }
    while let Some(hex) = queue.pop_front() {
        if border(hex) {
            continue;
        }
        for next in neighbours(hex) {
            if parent[next as usize] == -2 && !blocked[next as usize] {
                parent[next as usize] = hex;
                queue.push_back(next);
            }
        }
    }
    let reached = parent.iter().filter(|p| **p != -2).count();
    println!(
        "{path}: {} objects on elevation 0, {blockers} stop movement, {} hexes blocked",
        parsed.objects.elevations[0].len(),
        blocked.iter().filter(|b| **b).count()
    );
    println!("entry hex {entry}: {reached} hexes reachable");
    exits.sort_unstable();
    for &(hex, to, at) in &exits {
        if parent[hex as usize] == -2 {
            println!("exit {hex} -> map {to} hex {at}: NOT reachable from the entry");
            continue;
        }
        let mut route = vec![hex];
        while let Some(&last) = route.last() {
            let before = parent[last as usize];
            if before < 0 {
                break;
            }
            route.push(before);
        }
        route.reverse();
        let mut waypoints: Vec<i32> = route.iter().copied().skip(1).step_by(hop.max(1)).collect();
        if waypoints.last() != Some(&hex) {
            waypoints.push(hex);
        }
        println!(
            "exit {hex} -> map {to} hex {at}: {} steps, waypoints {waypoints:?}",
            route.len() - 1
        );
    }
    // Arrival points: what each reaches without stepping on an exit grid,
    // and whether that includes the entry hex. One that does not leaves the
    // walker no way on except back through an exit.
    let on_exit: Vec<bool> = {
        let mut on = vec![false; HEXES];
        for (hex, _, _) in &exits {
            on[*hex as usize] = true;
        }
        on
    };
    for &start in from {
        if !(0..HEXES as i32).contains(&start) || blocked[start as usize] {
            println!("from {start}: not a free hex");
            continue;
        }
        let mut seen = vec![false; HEXES];
        seen[start as usize] = true;
        let mut queue = VecDeque::from([start]);
        let mut count = 0usize;
        while let Some(hex) = queue.pop_front() {
            count += 1;
            if border(hex) {
                continue;
            }
            for next in neighbours(hex) {
                let n = next as usize;
                if !seen[n] && !blocked[n] && !on_exit[n] {
                    seen[n] = true;
                    queue.push_back(next);
                }
            }
        }
        let free = (0..HEXES as i32).contains(&entry) && seen[entry as usize];
        println!(
            "from {start}: {count} hexes without crossing an exit grid; entry {}",
            if free { "reachable" } else { "NOT reachable" }
        );
    }
    Ok(())
}
