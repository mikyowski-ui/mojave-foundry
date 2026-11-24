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

/// Walks a plugin, decodes every record through the typed tables (both of
/// them, `fnv_esm::records::all_decoders`), and tallies what did not decode
/// cleanly. `needle` selects cells by editor id substring
/// (case-insensitive) for a closer look; `cells` (an editor id substring
/// too) breaks the placed references of the matching cells down by the
/// record type of their base.
pub fn esm(path: &Path, needle: &str, cells: &str) -> Result<(), String> {
    use fnv_esm::{Plugin, Record};
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let plugin = Plugin::parse(&bytes).map_err(|e| format!("parse: {e}"))?;
    let header = plugin.header();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let game = match plugin.detect_game(&name, None) {
        Ok(game) => format!("{} (`{}`)", game.display_name(), game.id()),
        Err(error) => format!("unknown: {error}"),
    };
    println!(
        "plugin: version {}  game {game}  declared records {}  masters {:?}",
        header.version, header.record_count, header.masters
    );
    let breakdown = cells.to_ascii_lowercase();
    // Every record's signature by form id (as stored: one plugin's own
    // numbering), and the bases of the references in the broken-down cells.
    let mut sig_of: BTreeMap<u32, String> = BTreeMap::new();
    let mut broken_down: BTreeMap<u32, String> = BTreeMap::new();
    let mut bases: BTreeMap<u32, Vec<(String, Option<u32>)>> = BTreeMap::new();
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
        sig_of.insert(record.form_id().0, sig.clone());
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
                if !breakdown.is_empty() && id.to_ascii_lowercase().contains(&breakdown) {
                    let grid = c.grid.as_ref().map(|g| (g.x, g.y));
                    broken_down.insert(decoded.form_id().0, format!("{id} grid {grid:?}"));
                }
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
            Record::Placed(placed) => {
                if let Some(cell) = cell
                    && broken_down.contains_key(&cell)
                {
                    bases
                        .entry(cell)
                        .or_default()
                        .push((sig.clone(), placed.base.map(|b| b.0)));
                }
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
    let typed: Vec<(String, &str)> = fnv_esm::records::all_decoders()
        .map(|d| (d.sig.to_string(), d.type_name))
        .collect();
    println!("typed decoders: {}", typed.len());
    for (sig, type_name) in &typed {
        println!(
            "  {sig}: {}  ({type_name})",
            counts.get(sig).copied().unwrap_or(0)
        );
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
    if !breakdown.is_empty() {
        println!(
            "placed references by base record type, cells matching {breakdown:?}: {}",
            broken_down.len()
        );
        for (form, label) in &broken_down {
            let refs = bases.get(form).map_or(&[][..], Vec::as_slice);
            let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
            for (sig, base) in refs {
                let kind = match base {
                    Some(base) => sig_of
                        .get(base)
                        .cloned()
                        .unwrap_or_else(|| "(no record of this plugin)".to_owned()),
                    None => format!("({sig} without a base)"),
                };
                *by_kind.entry(kind).or_default() += 1;
            }
            let mut ranked: Vec<_> = by_kind.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            println!("  {form:08x} {label}: {} references", refs.len());
            for (kind, count) in ranked {
                println!("    {count:>6}  {kind}");
            }
        }
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
    // The AI packets a converted critter may name (the loot wiring picks
    // its hostile and friendly packets from these).
    match archive.read("data/ai.txt") {
        Ok(bytes) => match vault_formats::ai::Packets::parse_for(
            &bytes,
            vault_formats::GameProfile::Fallout2,
        ) {
            Ok(packets) => {
                println!(
                    "data/ai.txt: {} packets (number, name, min_hp, aggression, chance, \
                     min_to_hit, max_dist)",
                    packets.packets.len()
                );
                for p in &packets.packets {
                    println!(
                        "  {:>3} {:<28} min_hp {:>3} aggression {:>3} chance {:>3} to_hit {:>3} \
                         dist {:>3}",
                        p.packet_num,
                        String::from_utf8_lossy(&p.name),
                        p.min_hp,
                        p.aggression,
                        p.chance,
                        p.min_to_hit,
                        p.max_dist
                    );
                }
            }
            Err(e) => println!("data/ai.txt: does not parse: {e}"),
        },
        Err(e) => println!("data/ai.txt: {e}"),
    }
    Ok(())
}

/// Reads one map of a written mod layer back from disk and reports where a
/// walker can go, by the engine's movement rule ([`crate::route::WalkMap`]).
/// Prints, for each exit grid, whether it can be reached from the entry hex
/// and a chain of waypoints `hop` steps apart, short enough for the
/// engine's capped route search. For each hex in `from` (the points other
/// maps' exits arrive at) it then reports what can be reached without
/// stepping on an exit grid.
pub fn walk(
    layer_dir: &Path,
    map: &str,
    elevation: usize,
    hop: usize,
    from: &[i32],
) -> Result<(), String> {
    use crate::route::{self, HEXES, WalkMap};

    let walk_map = WalkMap::load_elevation(layer_dir, map, elevation)?;
    let path = &walk_map.path;
    let blocked = &walk_map.blocked;
    let entry = walk_map.entry;
    let parent = walk_map.flood_from_entry();
    let reached = parent.iter().filter(|p| **p != -2).count();
    println!(
        "{path}: {} objects on elevation {elevation}, {} stop movement, {} hexes blocked",
        walk_map.objects,
        walk_map.blockers,
        blocked.iter().filter(|b| **b).count()
    );
    if elevation == walk_map.entry_elevation {
        println!("entry hex {entry}: {reached} hexes reachable");
    } else {
        println!(
            "start hex {entry} (the first arrival of the map's stairs on elevation {elevation};              arrivals {:?}): {reached} hexes reachable",
            walk_map.arrivals
        );
    }
    for exit in &walk_map.exits {
        let (hex, to, at) = (exit.hex, exit.to_map, exit.to_hex);
        let at = if exit.to_elevation == 0 {
            at.to_string()
        } else {
            format!("{at} elevation {}", exit.to_elevation)
        };
        let route = route::path_to(&parent, hex);
        if route.is_empty() {
            println!("exit {hex} -> map {to} hex {at}: NOT reachable from the entry");
            continue;
        }
        let waypoints = route::waypoints(&route, hop);
        println!(
            "exit {hex} -> map {to} hex {at}: {} steps, waypoints {waypoints:?}",
            route.len() - 1
        );
    }
    // Arrival points: what each reaches without stepping on an exit grid,
    // and whether that includes the entry hex. One that does not leaves the
    // walker no way on except back through an exit.
    let on_exit = walk_map.exit_mask();
    for &start in from {
        if !(0..HEXES as i32).contains(&start) || blocked[start as usize] {
            println!("from {start}: not a free hex");
            continue;
        }
        // The start itself may be an exit grid's arrival hex, never an
        // exit grid: it is entered, not avoided.
        let mut avoid = on_exit.clone();
        avoid[start as usize] = false;
        let seen = route::flood(blocked, &avoid, start);
        let count = seen.iter().filter(|p| **p != -2).count();
        let free = route::reached(&seen, entry);
        println!(
            "from {start}: {count} hexes without crossing an exit grid; entry {}",
            if free { "reachable" } else { "NOT reachable" }
        );
    }
    Ok(())
}
