//! Calibration probe: runs the decoders over a real plugin and prints
//! structural tallies (counts, sizes, match rates). It never prints record
//! contents, and it writes nothing.
//!
//! ```text
//! FNV_ESM_PLUGIN=/path/to/plugin.esm cargo run -p fnv-esm --example calibrate
//! ```
//!
//! What it measures, and what the decoders assume as a result:
//!
//! - compressed records whose zlib checksum does not match their data;
//! - how often neighbouring cells agree along their shared land edge, under
//!   each of the four ways the height grid could face;
//! - the length and direction of land normals, and their lean against the
//!   slope of the heights;
//! - which of the 192 arrangements of texture quadrants and opacity
//!   positions makes a texture continuous across quadrant seams and across
//!   cell edges;
//! - navmesh counts, index ranges, edge links against their flag bits, and
//!   vertices against their cell's bounds;
//! - cells by interior flag, grid position and enclosing group, and block
//!   labels against grid positions;
//! - sizes and value ranges of the subrecords whose layouts were in doubt;
//! - which subrecords the typed decoders leave in `unknown`.

use std::collections::BTreeMap;

use fnv_esm::*;

type Tally<K> = BTreeMap<K, u64>;

fn bump<K: Ord>(tally: &mut Tally<K>, key: K) {
    *tally.entry(key).or_default() += 1;
}

fn le_u32(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

fn le_f32(data: &[u8], at: usize) -> f32 {
    f32::from_bits(le_u32(data, at))
}

#[derive(Default)]
struct CellInfo {
    world: Option<FormId>,
    grid: Option<(i32, i32)>,
}

#[derive(Default)]
struct Probe {
    records: u64,
    compressed: u64,
    bad_checksum: Tally<String>,
    record_errors: Tally<String>,
    unknown: Tally<(String, String, usize)>,
    // cells
    cells: BTreeMap<FormId, CellInfo>,
    cell_class: Tally<String>,
    block_labels: Tally<&'static str>,
    // land
    land: BTreeMap<(FormId, i32, i32), Land>,
    // navmesh
    nav: Tally<&'static str>,
    nav_data_sizes: Tally<usize>,
    // misc
    sizes: Tally<(String, String, usize)>,
    ctda: Tally<String>,
    armo: Tally<String>,
    ammo: Tally<String>,
    crea: Tally<String>,
    xloc: Tally<String>,
    refr: Tally<String>,
    forms: BTreeMap<FormId, Sig>,
    dat2_forms: Vec<(FormId, FormId)>,
    xloc_keys: Vec<FormId>,
    xemi_forms: Vec<FormId>,
    bad_checksum_cells: Vec<FormId>,
    bad_checksum_land: Vec<(FormId, i32, i32)>,
    nav_edges: Tally<String>,
    nav_triangles: BTreeMap<FormId, usize>,
    nav_external: Vec<(FormId, u16, u32)>,
    nav_doors: Vec<FormId>,
    misc: Tally<String>,
}

const SIZE_WATCH: &[(&[u8; 4], &[u8; 4])] = &[
    (b"ARMO", b"DNAM"),
    (b"ARMO", b"DATA"),
    (b"ARMO", b"BMDT"),
    (b"AMMO", b"DATA"),
    (b"AMMO", b"DAT2"),
    (b"CREA", b"DATA"),
    (b"NPC_", b"DATA"),
    (b"NPC_", b"DNAM"),
    (b"NPC_", b"ACBS"),
    (b"WEAP", b"DATA"),
    (b"WEAP", b"DNAM"),
    (b"WEAP", b"CRDT"),
    (b"REFR", b"XLOC"),
    (b"REFR", b"XTEL"),
    (b"REFR", b"XESP"),
    (b"REFR", b"XRDS"),
    (b"REFR", b"XPRM"),
    (b"REFR", b"XEMI"),
    (b"REFR", b"XLKR"),
    (b"REFR", b"DATA"),
    (b"ACHR", b"DATA"),
    (b"ACRE", b"DATA"),
    (b"CELL", b"DATA"),
    (b"CELL", b"XCLC"),
    (b"CELL", b"XCLL"),
    (b"WRLD", b"DATA"),
    (b"WRLD", b"MNAM"),
    (b"WRLD", b"ONAM"),
    (b"WRLD", b"NAM0"),
    (b"WRLD", b"DNAM"),
    (b"NAVM", b"DATA"),
    (b"QUST", b"DATA"),
    (b"QUST", b"INDX"),
    (b"QUST", b"QSTA"),
    (b"INFO", b"DATA"),
    (b"INFO", b"TRDT"),
    (b"DIAL", b"DATA"),
    (b"SCPT", b"SCHR"),
    (b"SCPT", b"SLSD"),
    (b"ALCH", b"ENIT"),
    (b"ALCH", b"EFIT"),
    (b"CONT", b"DATA"),
    (b"MISC", b"DATA"),
    (b"LAND", b"VHGT"),
    (b"LAND", b"VNML"),
    (b"LAND", b"VCLR"),
    (b"LAND", b"BTXT"),
    (b"LAND", b"ATXT"),
    (b"LTEX", b"HNAM"),
    (b"TXST", b"DNAM"),
];

impl Probe {
    fn record(&mut self, record: &RecordRef<'_>, context: &Context) {
        self.records += 1;
        let sig = record.sig();
        let name = sig.to_string();
        self.forms.insert(record.form_id(), sig);
        let data = match record.data() {
            Ok(data) => data,
            Err(error) => {
                let kind = format!(
                    "{name}: {}",
                    error.to_string().split(": ").nth(1).unwrap_or("?")
                );
                bump(&mut self.record_errors, kind);
                return;
            }
        };
        if data.was_compressed() {
            self.compressed += 1;
            if !data.checksum_ok() {
                bump(&mut self.bad_checksum, name.clone());
                self.bad_checksum_cells.extend(context.cell());
            }
        }
        let subs = match data.collect() {
            Ok(subs) => subs,
            Err(_) => {
                bump(
                    &mut self.record_errors,
                    format!("{name}: subrecord framing"),
                );
                return;
            }
        };
        for sub in &subs {
            if SIZE_WATCH
                .iter()
                .any(|(r, s)| **r == sig.0 && **s == sub.sig.0)
            {
                bump(
                    &mut self.sizes,
                    (name.clone(), sub.sig.to_string(), sub.data.len()),
                );
            }
            if &sub.sig.0 == b"CTDA" {
                self.ctda(sub.data);
            }
        }
        let decoded = decode_record(sig, record.header.meta, &subs);
        for unknown in decoded.unknown_subrecords() {
            if !matches!(decoded, Record::Unknown(_)) {
                bump(
                    &mut self.unknown,
                    (
                        name.clone(),
                        unknown.sig.to_string(),
                        unknown.data.len().min(9999),
                    ),
                );
            }
        }
        match decoded {
            Record::Cell(cell) => self.cell(&cell, context),
            Record::Land(land) => {
                if let Some(cell) = context.cell().and_then(|c| self.cells.get(&c))
                    && let (Some(world), Some((x, y))) = (cell.world, cell.grid)
                {
                    if context
                        .cell()
                        .is_some_and(|c| self.bad_checksum_cells.contains(&c))
                    {
                        self.bad_checksum_land.push((world, x, y));
                    }
                    self.land.insert((world, x, y), land);
                }
            }
            Record::Navmesh(navmesh) => self.navmesh(&navmesh, &subs, context),
            Record::Armor(armor) => {
                if let Some(stats) = armor.stats {
                    bump(
                        &mut self.armo,
                        match stats.damage_threshold {
                            Some(dt) if (0.0..=100.0).contains(&dt) => "dt in 0..=100".into(),
                            Some(_) => "dt out of range".into(),
                            None => "no dt".into(),
                        },
                    );
                    bump(&mut self.armo, format!("flags {:#x}", stats.flags));
                    bump(
                        &mut self.armo,
                        format!("ar/100 bucket {}", stats.armor_rating / 1000),
                    );
                }
                for sub in &subs {
                    if &sub.sig.0 == b"DNAM" && sub.data.len() == 12 {
                        let pad = sub.data[2] != 0 || sub.data[3] != 0;
                        let tail = sub.data[10] != 0 || sub.data[11] != 0;
                        bump(
                            &mut self.armo,
                            format!("pad2 nonzero {pad} tail nonzero {tail}"),
                        );
                    }
                }
            }
            Record::Ammo(ammo) => {
                if let Some(d) = ammo.data2 {
                    bump(
                        &mut self.ammo,
                        format!(
                            "per-shot {} weight {} pct {}",
                            d.projectiles_per_shot.min(99),
                            if (0.0..10.0).contains(&d.weight) {
                                "0..10"
                            } else {
                                "odd"
                            },
                            if (0.0..=1.0).contains(&d.consumed_percentage) {
                                "0..=1"
                            } else {
                                "odd"
                            },
                        ),
                    );
                    self.dat2_forms.push((d.projectile, d.consumed_ammo));
                    bump(
                        &mut self.misc,
                        format!(
                            "DAT2 pct bucket {}",
                            (d.consumed_percentage / 5.0).floor() * 5.0
                        ),
                    );
                }
                if let Some(d) = ammo.data {
                    bump(
                        &mut self.ammo,
                        format!(
                            "data: speed {} value {} flags {:#x}",
                            if d.speed.is_finite() && d.speed >= 0.0 {
                                "ok"
                            } else {
                                "odd"
                            },
                            if (0..100_000).contains(&d.value) {
                                "ok"
                            } else {
                                "odd"
                            },
                            d.flags
                        ),
                    );
                }
            }
            Record::Creature(creature) => {
                if let Some(d) = creature.data {
                    if !(0..=5000).contains(&d.health) {
                        bump(
                            &mut self.misc,
                            format!("CREA odd health sign {}", d.health.signum()),
                        );
                    }
                    bump(&mut self.crea, format!("type {}", d.creature_type));
                    bump(
                        &mut self.crea,
                        format!(
                            "health {} damage {} attrs<=10 {}",
                            if (0..=5000).contains(&d.health) {
                                "0..=5000"
                            } else {
                                "odd"
                            },
                            if (0..=1000).contains(&d.damage) {
                                "0..=1000"
                            } else {
                                "odd"
                            },
                            d.attributes.iter().all(|&a| a <= 10)
                        ),
                    );
                }
                for sub in &subs {
                    if &sub.sig.0 == b"DATA" && sub.data.len() == 17 {
                        let unused = sub.data[6] != 0 || sub.data[7] != 0;
                        bump(&mut self.crea, format!("bytes 6..8 nonzero {unused}"));
                    }
                }
            }
            Record::Placed(placed) => self.placed(&placed, &subs),
            _ => {}
        }
    }

    fn ctda(&mut self, data: &[u8]) {
        bump(&mut self.ctda, format!("size {}", data.len()));
        if data.len() >= 12 {
            let function = le_u32(data, 8);
            bump(
                &mut self.ctda,
                format!(
                    "function high16 {}",
                    if function >> 16 == 0 {
                        "zero"
                    } else {
                        "NONZERO"
                    }
                ),
            );
            bump(
                &mut self.ctda,
                format!("function low16 bucket {}", (function & 0xFFFF) / 1000),
            );
            bump(&mut self.ctda, format!("comparison code {}", data[0] >> 5));
            bump(&mut self.ctda, format!("flag bits {:#04x}", data[0] & 0x1F));
            bump(
                &mut self.ctda,
                format!(
                    "pad bytes {}",
                    if data[1..4].iter().all(|&b| b == 0) {
                        "zero"
                    } else {
                        "nonzero"
                    }
                ),
            );
        }
        if data.len() >= 24 {
            bump(
                &mut self.ctda,
                format!("run-on {}", le_u32(data, 20).min(99)),
            );
        }
    }

    fn cell(&mut self, cell: &Cell, context: &Context) {
        let in_world = context.worldspace().is_some();
        let top = context.top().map(|s| s.to_string()).unwrap_or_default();
        let class = format!(
            "top {top} flag_interior {} has_grid {} has_DATA {} direct_world_child {}",
            cell.is_interior(),
            cell.grid.is_some(),
            cell.flags.is_some(),
            in_world && context.exterior_block().is_none(),
        );
        bump(&mut self.cell_class, class);
        if let (Some(grid), Some((bx, by)), Some((sx, sy))) = (
            cell.grid,
            context.exterior_block(),
            context.exterior_sub_block(),
        ) {
            let expect_block = (grid.x.div_euclid(32), grid.y.div_euclid(32));
            let expect_sub = (grid.x.div_euclid(8), grid.y.div_euclid(8));
            let ok = (i32::from(bx), i32::from(by)) == expect_block
                && (i32::from(sx), i32::from(sy)) == expect_sub;
            let swapped = (i32::from(by), i32::from(bx)) == expect_block
                && (i32::from(sy), i32::from(sx)) == expect_sub;
            bump(
                &mut self.block_labels,
                match (ok, swapped) {
                    (true, true) => "both orders fit",
                    (true, false) => "label is (y, x): fits",
                    (false, true) => "label is (x, y): fits",
                    (false, false) => "neither fits",
                },
            );
        }
        self.cells.insert(
            cell.common.form_id(),
            CellInfo {
                world: context.worldspace(),
                grid: cell.grid.map(|g| (g.x, g.y)),
            },
        );
    }

    fn navmesh(&mut self, navmesh: &Navmesh, subs: &[SubrecordRef<'_>], context: &Context) {
        bump(&mut self.nav, "navmeshes");
        self.nav_triangles
            .insert(navmesh.common.form_id(), navmesh.triangles.len());
        for link in &navmesh.external_links {
            self.nav_external
                .push((link.navmesh, link.triangle, link.unknown));
        }
        for link in &navmesh.door_links {
            self.nav_doors.push(link.door);
            bump(
                &mut self.nav,
                if usize::from(link.triangle) < navmesh.triangles.len() {
                    "door link triangle in range"
                } else {
                    "door link triangle OUT of range"
                },
            );
        }
        for &cover in &navmesh.cover_triangles {
            bump(
                &mut self.nav,
                if usize::from(cover) < navmesh.triangles.len() {
                    "cover triangle in range"
                } else {
                    "cover triangle OUT of range"
                },
            );
        }
        let vertex_count = navmesh.vertices.len();
        let triangle_count = navmesh.triangles.len();
        if navmesh.cell != context.cell() {
            bump(&mut self.nav, "DATA cell differs from group cell");
        }
        for sub in subs {
            if &sub.sig.0 != b"DATA" {
                continue;
            }
            bump(&mut self.nav_data_sizes, sub.data.len());
            if sub.data.len() == 24 {
                let counts: Vec<usize> = (1..6).map(|i| le_u32(sub.data, i * 4) as usize).collect();
                let actual = [
                    vertex_count,
                    triangle_count,
                    navmesh.external_links.len(),
                    navmesh.cover_triangles.len(),
                    navmesh.door_links.len(),
                ];
                const OK: [&str; 5] = [
                    "count ok: vertices",
                    "count ok: triangles",
                    "count ok: external",
                    "count ok: cover",
                    "count ok: doors",
                ];
                const BAD: [&str; 5] = [
                    "count BAD: vertices",
                    "count BAD: triangles",
                    "count BAD: external",
                    "count BAD: cover",
                    "count BAD: doors",
                ];
                for (i, (count, actual)) in counts.iter().zip(actual).enumerate() {
                    bump(&mut self.nav, if *count == actual { OK[i] } else { BAD[i] });
                }
            }
        }
        let mut bad_vertex = false;
        let mut bad_edge = false;
        for triangle in &navmesh.triangles {
            for &v in &triangle.vertices {
                if usize::try_from(v).map_or(true, |v| v >= vertex_count) {
                    bad_vertex = true;
                }
            }
            for (i, &e) in triangle.edges.iter().enumerate() {
                if e == -1 {
                    bump(&mut self.nav, "edges: -1");
                } else if usize::try_from(e).is_ok_and(|e| e < triangle_count) {
                    bump(&mut self.nav, "edges: in triangle range");
                } else {
                    bad_edge = true;
                    bump(&mut self.nav, "edges: OUT of triangle range");
                }
                let bit = triangle.flags >> i & 1;
                let class = if e == -1 {
                    "none"
                } else if usize::try_from(e).is_ok_and(|e| e < triangle_count) {
                    let other = &navmesh.triangles[e as usize];
                    let a = triangle.vertices[i];
                    let b = triangle.vertices[(i + 1) % 3];
                    if other.vertices.contains(&a) && other.vertices.contains(&b) {
                        "triangle sharing the edge"
                    } else {
                        "in triangle range, not sharing"
                    }
                } else {
                    "beyond triangle range"
                };
                let ext = usize::try_from(e).is_ok_and(|e| e < navmesh.external_links.len());
                bump(
                    &mut self.nav_edges,
                    format!("flag bit {bit}: {class}; index < NVEX count {ext}"),
                );
            }
        }
        if bad_vertex {
            bump(&mut self.nav, "navmeshes with a vertex index out of range");
        }
        if bad_edge {
            bump(&mut self.nav, "navmeshes with an edge out of range");
        }
        // Reciprocity: if edge i of t names u, some edge of u names t.
        let mut reciprocal = 0u64;
        let mut one_way = 0u64;
        for (t, triangle) in navmesh.triangles.iter().enumerate() {
            for &e in &triangle.edges {
                if let Some(other) = usize::try_from(e)
                    .ok()
                    .and_then(|e| navmesh.triangles.get(e))
                {
                    if other
                        .edges
                        .iter()
                        .any(|&back| usize::try_from(back) == Ok(t))
                    {
                        reciprocal += 1;
                    } else {
                        one_way += 1;
                    }
                }
            }
        }
        *self.nav.entry("edge links reciprocated").or_default() += reciprocal;
        *self.nav.entry("edge links one-way").or_default() += one_way;
        // Shared-edge geometry: edge i (vertex i -> i+1) of t vs neighbour.
        let mut share = [0u64; 3];
        let mut total = 0u64;
        for triangle in &navmesh.triangles {
            for (i, &e) in triangle.edges.iter().enumerate() {
                let Some(other) = usize::try_from(e)
                    .ok()
                    .and_then(|e| navmesh.triangles.get(e))
                else {
                    continue;
                };
                total += 1;
                for (k, share_count) in share.iter_mut().enumerate() {
                    // Hypothesis k: edge i joins vertices (i+k) and (i+k+1).
                    let a = triangle.vertices[(i + k) % 3];
                    let b = triangle.vertices[(i + k + 1) % 3];
                    if other.vertices.contains(&a) && other.vertices.contains(&b) {
                        *share_count += 1;
                    }
                }
            }
        }
        *self.nav.entry("edge geometry: links checked").or_default() += total;
        *self
            .nav
            .entry("edge geometry: edge i = v[i]..v[i+1]")
            .or_default() += share[0];
        *self
            .nav
            .entry("edge geometry: edge i = v[i+1]..v[i+2]")
            .or_default() += share[1];
        *self
            .nav
            .entry("edge geometry: edge i = v[i+2]..v[i]")
            .or_default() += share[2];

        if let Some(info) = context.cell().and_then(|c| self.cells.get(&c)) {
            if let Some((x, y)) = info.grid {
                let (x0, y0) = (x as f32 * 4096.0, y as f32 * 4096.0);
                let mut inside = 0u64;
                let mut near = 0u64;
                let mut far = 0u64;
                for v in &navmesh.vertices {
                    let dx = (x0 - v[0]).max(v[0] - (x0 + 4096.0)).max(0.0);
                    let dy = (y0 - v[1]).max(v[1] - (y0 + 4096.0)).max(0.0);
                    let d = dx.max(dy);
                    if d == 0.0 {
                        inside += 1;
                    } else if d <= 256.0 {
                        near += 1;
                    } else {
                        far += 1;
                    }
                }
                *self.nav.entry("exterior vertices inside cell").or_default() += inside;
                *self
                    .nav
                    .entry("exterior vertices within 256 of cell")
                    .or_default() += near;
                *self.nav.entry("exterior vertices farther out").or_default() += far;
                if far > 0 {
                    bump(&mut self.nav, "exterior navmeshes with a far vertex");
                }
            } else {
                bump(&mut self.nav, "navmeshes in cells without a grid");
            }
        }
        if !navmesh.vertices.iter().flatten().all(|c| c.is_finite()) {
            bump(&mut self.nav, "navmeshes with a non-finite vertex");
        }
    }

    fn placed(&mut self, placed: &PlacedRef, subs: &[SubrecordRef<'_>]) {
        if placed.kind != PlacedKind::Object {
            return;
        }
        for sub in subs {
            let d = sub.data;
            match &sub.sig.0 {
                b"XLOC" if d.len() >= 12 => {
                    bump(&mut self.xloc, format!("level {}", d[0]));
                    bump(&mut self.xloc, format!("flags {:#x}", d[8]));
                    bump(
                        &mut self.xloc,
                        format!(
                            "pad 1..4 zero {} pad 9..12 zero {} tail zero {}",
                            d[1..4].iter().all(|&b| b == 0),
                            d[9..12].iter().all(|&b| b == 0),
                            d[12..].iter().all(|&b| b == 0)
                        ),
                    );
                    self.xloc_keys.push(FormId(le_u32(d, 4)));
                }
                b"XRDS" if d.len() == 4 => {
                    let r = le_f32(d, 0);
                    let base = placed
                        .base
                        .and_then(|b| self.forms.get(&b))
                        .map_or("?".to_owned(), |s| s.to_string());
                    bump(
                        &mut self.misc,
                        format!(
                            "XRDS base {base} {}",
                            if r < 0.0 {
                                "negative"
                            } else if r == 0.0 {
                                "zero"
                            } else {
                                "positive"
                            }
                        ),
                    );
                    bump(
                        &mut self.misc,
                        format!(
                            "XRDS {}",
                            if !r.is_finite() {
                                "non-finite".to_owned()
                            } else if r < 0.0 {
                                "negative".to_owned()
                            } else {
                                format!("1e{}", (r.max(1.0).log10().floor()))
                            }
                        ),
                    );
                    bump(
                        &mut self.refr,
                        format!(
                            "XRDS {}",
                            if r.is_finite() && (0.0..100_000.0).contains(&r) {
                                "finite 0..1e5"
                            } else {
                                "odd"
                            }
                        ),
                    );
                }
                b"XPRM" if d.len() == 32 => {
                    let bounds = [le_f32(d, 0), le_f32(d, 4), le_f32(d, 8)];
                    let color = [le_f32(d, 12), le_f32(d, 16), le_f32(d, 20)];
                    bump(
                        &mut self.refr,
                        format!(
                            "XPRM bounds {} colour0..1 {} unk-as-f32 {} type {}",
                            if bounds
                                .iter()
                                .all(|b| b.is_finite() && (0.0..100_000.0).contains(b))
                            {
                                "finite>=0"
                            } else {
                                "odd"
                            },
                            color.iter().all(|c| (0.0..=1.0).contains(c)),
                            if (0.0..=1.0).contains(&le_f32(d, 24)) {
                                "0..=1"
                            } else {
                                "other"
                            },
                            le_u32(d, 28).min(99)
                        ),
                    );
                }
                b"XEMI" if d.len() == 4 => self.xemi_forms.push(FormId(le_u32(d, 0))),
                b"XTEL" if d.len() == 32 => {
                    bump(&mut self.refr, format!("XTEL flags {:#x}", le_u32(d, 28)));
                }
                b"XESP" if d.len() == 8 => {
                    bump(
                        &mut self.refr,
                        format!(
                            "XESP flags {:#x} pad zero {}",
                            d[4],
                            d[5..8].iter().all(|&b| b == 0)
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}

/// Edge-match statistics for one worldspace under one orientation.
fn edge_match(
    land: &BTreeMap<(FormId, i32, i32), Land>,
    world: FormId,
    rows_from_south: bool,
    columns_from_west: bool,
) -> (u64, u64, u64, u64) {
    let at = |heights: &[f32], row: usize, column: usize| {
        let r = if rows_from_south { row } else { 32 - row };
        let c = if columns_from_west {
            column
        } else {
            32 - column
        };
        heights[r * 33 + c]
    };
    let (mut pairs, mut exact_pairs, mut vertices, mut exact) = (0u64, 0u64, 0u64, 0u64);
    for (&(w, x, y), here) in land.range((world, i32::MIN, i32::MIN)..=(world, i32::MAX, i32::MAX))
    {
        let Some(here) = here.heights.as_deref() else {
            continue;
        };
        let east = land.get(&(w, x + 1, y)).and_then(|l| l.heights.as_deref());
        let north = land.get(&(w, x, y + 1)).and_then(|l| l.heights.as_deref());
        if let Some(east) = east {
            pairs += 1;
            let same = (0..33)
                .filter(|&i| at(here, i, 32) == at(east, i, 0))
                .count() as u64;
            vertices += 33;
            exact += same;
            exact_pairs += u64::from(same == 33);
        }
        if let Some(north) = north {
            pairs += 1;
            let same = (0..33)
                .filter(|&i| at(here, 32, i) == at(north, 0, i))
                .count() as u64;
            vertices += 33;
            exact += same;
            exact_pairs += u64::from(same == 33);
        }
    }
    (pairs, exact_pairs, vertices, exact)
}

fn mismatch_report(probe: &Probe) {
    let mut tally: Tally<String> = Tally::new();
    for (&(w, x, y), here_land) in &probe.land {
        let Some(here) = here_land.heights.as_deref() else {
            continue;
        };
        let mut check = |other: (FormId, i32, i32),
                         mine: Vec<f32>,
                         pick: &dyn Fn(&[f32]) -> Vec<f32>| {
            let Some(there) = probe.land.get(&other).and_then(|l| l.heights.as_deref()) else {
                return;
            };
            let theirs = pick(there);
            let diffs: Vec<f32> = mine.iter().zip(&theirs).map(|(a, b)| a - b).collect();
            let differing = diffs.iter().filter(|d| **d != 0.0).count();
            if differing == 0 {
                return;
            }
            let constant = diffs.iter().all(|d| *d == diffs[0]);
            let max = diffs.iter().fold(0f32, |m, d| m.max(d.abs()));
            let bad = probe.bad_checksum_land.contains(&(w, x, y))
                || probe.bad_checksum_land.contains(&other);
            let multiple = diffs.iter().all(|d| d % 8.0 == 0.0);
            bump(
                &mut tally,
                format!(
                    "world {w}: differing {} constant {constant} max {} multiples-of-8 {multiple} bad-checksum-cell {bad}",
                    if differing == 33 {
                        "all"
                    } else if differing == 1 {
                        "one"
                    } else {
                        "some"
                    },
                    if max <= 8.0 {
                        "<=8"
                    } else if max <= 64.0 {
                        "<=64"
                    } else if max <= 512.0 {
                        "<=512"
                    } else {
                        ">512"
                    },
                ),
            );
        };
        let east_edge: Vec<f32> = (0..33).map(|r| here[r * 33 + 32]).collect();
        check((w, x + 1, y), east_edge, &|t| {
            (0..33).map(|r| t[r * 33]).collect()
        });
        let north_edge: Vec<f32> = (0..33).map(|c| here[32 * 33 + c]).collect();
        check((w, x, y + 1), north_edge, &|t| {
            (0..33).map(|c| t[c]).collect()
        });
    }
    println!("\n== unequal edge pairs ==");
    for (what, count) in &tally {
        println!("  {count:>5}  {what}");
    }
    println!("bad-checksum land keys: {}", probe.bad_checksum_land.len());
}

fn land_report(probe: &Probe) {
    let mut per_world: Tally<FormId> = Tally::new();
    for &(world, _, _) in probe.land.keys() {
        bump(&mut per_world, world);
    }
    println!("\n== LAND edge matching (pairs, fully equal pairs, vertices, equal vertices) ==");
    for (&world, &count) in &per_world {
        println!("worldspace {world}: {count} land records");
        for (south, west) in [(true, true), (false, true), (true, false), (false, false)] {
            let (pairs, exact_pairs, vertices, exact) = edge_match(&probe.land, world, south, west);
            if pairs == 0 {
                continue;
            }
            println!(
                "  row0={} col0={}: pairs {pairs} equal {exact_pairs} ({:.2}%), vertices {vertices} equal {exact} ({:.3}%)",
                if south { "south" } else { "north" },
                if west { "west" } else { "east" },
                100.0 * exact_pairs as f64 / pairs as f64,
                100.0 * exact as f64 / vertices as f64
            );
        }
    }

    // Heights: all multiples of 8 away from the offset by construction;
    // report the range.
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut without = 0;
    for land in probe.land.values() {
        match &land.heights {
            Some(h) => {
                for &v in h {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            None => without += 1,
        }
    }
    println!("height range {lo}..{hi}; lands without VHGT {without}");

    println!("\n== VNML / VCLR ==");
    let mut length: Tally<i64> = Tally::new();
    let mut up: Tally<&'static str> = Tally::new();
    let mut unsigned_length: Tally<i64> = Tally::new();
    let mut colors: Tally<&'static str> = Tally::new();
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    for land in probe.land.values() {
        match &land.normals {
            Some(normals) => {
                for v in normals {
                    let f = v.map(f64::from);
                    let len = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
                    bump(&mut length, (len / 4.0).round() as i64 * 4);
                    bump(
                        &mut up,
                        if v[2] > 0
                            && v[2].unsigned_abs() >= v[0].unsigned_abs().max(v[1].unsigned_abs())
                        {
                            "z positive and dominant"
                        } else if v[2] > 0 {
                            "z positive, not dominant"
                        } else {
                            "z <= 0"
                        },
                    );
                    let u = v.map(|b| f64::from(b as u8));
                    let ulen = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
                    bump(&mut unsigned_length, (ulen / 32.0).round() as i64 * 32);
                    for i in 0..3 {
                        sum[i] += f[i];
                    }
                    n += 1.0;
                }
            }
            None => bump(&mut up, "lands without VNML"),
        }
        match &land.colors {
            Some(c) => {
                bump(&mut colors, "lands with VCLR");
                if c.iter().all(|c| *c == [255, 255, 255]) {
                    bump(&mut colors, "lands with all-white VCLR");
                }
                for c in c {
                    bump(
                        &mut colors,
                        if *c == [255, 255, 255] {
                            "vertices white"
                        } else if c[0] == c[1] && c[1] == c[2] {
                            "vertices grey"
                        } else {
                            "vertices tinted"
                        },
                    );
                }
            }
            None => bump(&mut colors, "lands without VCLR"),
        }
    }
    println!("signed length histogram (bucket of 4): {length:?}");
    println!("unsigned-reading length histogram (bucket of 32): {unsigned_length:?}");
    println!("direction: {up:?}");
    println!(
        "mean signed components: {:?}",
        sum.map(|s| (s / n * 100.0).round() / 100.0)
    );
    println!("colours: {colors:?}");

    // Normal vs height gradient: the normal's x should oppose dh/dx (east),
    // its y oppose dh/dy (north) if x=east, y=north and the grid is
    // row-south/column-west.
    let mut agree: Tally<&'static str> = Tally::new();
    for land in probe.land.values() {
        let (Some(h), Some(nm)) = (&land.heights, &land.normals) else {
            continue;
        };
        for row in 1..32 {
            for column in 1..32 {
                let i = row * 33 + column;
                let dx = h[i + 1] - h[i - 1];
                let dy = h[i + 33] - h[i - 33];
                let v = nm[i];
                if dx.abs() >= 64.0 && v[0] != 0 {
                    bump(
                        &mut agree,
                        if (dx > 0.0) == (v[0] < 0) {
                            "x opposes dh/d(column)"
                        } else {
                            "x FOLLOWS dh/d(column)"
                        },
                    );
                }
                if dy.abs() >= 64.0 && v[1] != 0 {
                    bump(
                        &mut agree,
                        if (dy > 0.0) == (v[1] < 0) {
                            "y opposes dh/d(row)"
                        } else {
                            "y FOLLOWS dh/d(row)"
                        },
                    );
                }
            }
        }
    }
    println!("normal vs slope: {agree:?}");
}

/// The eight ways a 17 x 17 position could map to (row from south, column
/// from west).
fn position(variant: usize, row: usize, column: usize) -> usize {
    let (r, c) = if variant & 4 != 0 {
        (column, row)
    } else {
        (row, column)
    };
    let r = if variant & 1 != 0 { 16 - r } else { r };
    let c = if variant & 2 != 0 { 16 - c } else { c };
    r * 17 + c
}

const CORNERS: [&str; 4] = ["SW", "SE", "NW", "NE"];

fn quadrant_report(probe: &Probe) {
    println!("\n== Texture quadrants ==");
    let mut quadrants: Tally<String> = Tally::new();
    let mut positions: Tally<&'static str> = Tally::new();
    let mut entries: Tally<usize> = Tally::new();
    // Per land: quadrant index -> texture -> 289 opacities.
    type Layers = BTreeMap<(u8, FormId), Vec<f32>>;
    let mut lands: Vec<((FormId, i32, i32), Layers)> = Vec::new();
    for (key, land) in &probe.land {
        let mut layers: Layers = BTreeMap::new();
        for base in &land.base_layers {
            bump(
                &mut quadrants,
                format!("BTXT quadrant {} layer {}", base.quadrant, base.layer),
            );
        }
        for alpha in &land.alpha_layers {
            bump(&mut quadrants, format!("ATXT quadrant {}", alpha.quadrant));
            bump(
                &mut quadrants,
                format!("ATXT layer {}", alpha.layer.min(20)),
            );
            bump(&mut entries, alpha.opacity.len().min(290));
            let mut grid = vec![0.0f32; 289];
            for point in &alpha.opacity {
                match grid.get_mut(usize::from(point.position)) {
                    Some(slot) => {
                        *slot = point.opacity;
                        bump(&mut positions, "position < 289");
                    }
                    None => bump(&mut positions, "position >= 289"),
                }
                bump(
                    &mut positions,
                    if (0.0..=1.0).contains(&point.opacity) {
                        "opacity in 0..=1"
                    } else {
                        "opacity out of range"
                    },
                );
            }
            if alpha.quadrant < 4 {
                layers.insert((alpha.quadrant, alpha.texture), grid);
            }
        }
        if !layers.is_empty() {
            lands.push((*key, layers));
        }
    }
    println!("{quadrants:?}");
    println!("{positions:?}");
    let full = entries.get(&289).copied().unwrap_or(0);
    println!(
        "VTXT entry counts: {} layers, {} with all 289, max bucket {:?}",
        entries.values().sum::<u64>(),
        full,
        entries.keys().last()
    );

    // Seams inside a cell. For a mapping `corner_of[q]` and a position
    // variant, sum |difference| of the same texture's opacity along the
    // four internal seams.
    let mut results: Vec<(f64, u64, [usize; 4], usize)> = Vec::new();
    let mut perm = [0usize, 1, 2, 3];
    let mut perms = Vec::new();
    permute(&mut perm, 0, &mut perms);
    for corner_of in &perms {
        for variant in 0..8 {
            let mut total = 0f64;
            let mut count = 0u64;
            for (_, layers) in lands.iter().step_by(3) {
                for (&(qa, texture), a) in layers {
                    for qb in 0..4u8 {
                        if qb == qa {
                            continue;
                        }
                        let Some(b) = layers.get(&(qb, texture)) else {
                            continue;
                        };
                        let (ca, cb) = (corner_of[usize::from(qa)], corner_of[usize::from(qb)]);
                        // corners: 0 SW, 1 SE, 2 NW, 3 NE.
                        let seam: Option<(bool, bool)> = match (ca, cb) {
                            (0, 1) | (2, 3) => Some((true, true)),  // a west of b
                            (0, 2) | (1, 3) => Some((false, true)), // a south of b
                            _ => None,
                        };
                        let Some((vertical, _)) = seam else { continue };
                        for i in 0..17 {
                            let (pa, pb) = if vertical {
                                (position(variant, i, 16), position(variant, i, 0))
                            } else {
                                (position(variant, 16, i), position(variant, 0, i))
                            };
                            total += f64::from((a[pa] - b[pb]).abs());
                            count += 1;
                        }
                    }
                }
            }
            results.push((total / count.max(1) as f64, count, *corner_of, variant));
        }
    }
    results.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!(
        "internal seams, best 6 and worst 2 of {} hypotheses (mean |diff|, samples, index->corner, position variant):",
        results.len()
    );
    let describe = |variant: usize| {
        format!(
            "{}{}{}",
            if variant & 4 != 0 {
                "column-major "
            } else {
                "row-major "
            },
            if variant & 1 != 0 {
                "first-index-reversed "
            } else {
                ""
            },
            if variant & 2 != 0 {
                "second-index-reversed"
            } else {
                ""
            }
        )
    };
    for (mean, count, corner_of, variant) in
        results.iter().take(6).chain(results.iter().rev().take(2))
    {
        let names: Vec<&str> = corner_of.iter().map(|&c| CORNERS[c]).collect();
        println!(
            "  {mean:.5}  n={count}  0..3 -> {names:?}  {}",
            describe(*variant)
        );
    }

    // Seams across cell edges, every hypothesis: east-side quadrants of
    // (x, y) against west-side quadrants of (x + 1, y) at the same
    // north/south half; north-side quadrants against south-side quadrants
    // of (x, y + 1) at the same east/west half.
    let by_key: BTreeMap<(FormId, i32, i32), &Layers> =
        lands.iter().map(|(k, l)| (*k, l)).collect();
    let mut across: Vec<(f64, f64, u64, [usize; 4], usize)> = Vec::new();
    for corner_of in &perms {
        let index_of =
            |corner: usize| corner_of.iter().position(|&c| c == corner).unwrap_or(0) as u8;
        for variant in 0..8 {
            let mut total = 0f64;
            let mut equal = 0u64;
            let mut count = 0u64;
            for (&(world, x, y), layers) in by_key.iter().step_by(2) {
                let mut seam =
                    |other: Option<&&Layers>, pairs: [(usize, usize); 2], vertical: bool| {
                        let Some(other) = other else { return };
                        for (ca, cb) in pairs {
                            let (qa, qb) = (index_of(ca), index_of(cb));
                            for (&(q, texture), a) in layers.iter() {
                                if q != qa {
                                    continue;
                                }
                                let Some(b) = other.get(&(qb, texture)) else {
                                    continue;
                                };
                                for i in 0..17 {
                                    let (pa, pb) = if vertical {
                                        (position(variant, i, 16), position(variant, i, 0))
                                    } else {
                                        (position(variant, 16, i), position(variant, 0, i))
                                    };
                                    total += f64::from((a[pa] - b[pb]).abs());
                                    equal += u64::from(a[pa] == b[pb]);
                                    count += 1;
                                }
                            }
                        }
                    };
                // corners: 0 SW, 1 SE, 2 NW, 3 NE.
                seam(by_key.get(&(world, x + 1, y)), [(1, 0), (3, 2)], true);
                seam(by_key.get(&(world, x, y + 1)), [(2, 0), (3, 1)], false);
            }
            across.push((
                total / count.max(1) as f64,
                100.0 * equal as f64 / count.max(1) as f64,
                count,
                *corner_of,
                variant,
            ));
        }
    }
    across.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!(
        "across cell edges, best 5 of {} hypotheses (mean |diff|, % exactly equal, samples):",
        across.len()
    );
    for (mean, equal, count, corner_of, variant) in across.iter().take(5) {
        let names: Vec<&str> = corner_of.iter().map(|&c| CORNERS[c]).collect();
        println!(
            "  {mean:.5}  {equal:.2}%  n={count}  0..3 -> {names:?}  {}",
            describe(*variant)
        );
    }
}

fn permute(items: &mut [usize; 4], start: usize, out: &mut Vec<[usize; 4]>) {
    if start == 4 {
        out.push(*items);
        return;
    }
    for i in start..4 {
        items.swap(start, i);
        permute(items, start + 1, out);
        items.swap(start, i);
    }
}

fn main() {
    let path = std::env::var("FNV_ESM_PLUGIN").expect("set FNV_ESM_PLUGIN to a plugin path");
    let bytes = std::fs::read(path).expect("read plugin");
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut probe = Probe::default();
    loop {
        match walker.next_event() {
            Ok(Some(Event::Record(record))) => probe.record(&record, walker.context()),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => {
                println!("walk error: {error}");
                break;
            }
        }
    }
    println!("records {} compressed {}", probe.records, probe.compressed);
    println!(
        "records with a wrong zlib checksum: {:?}",
        probe.bad_checksum
    );
    println!("record errors: {:?}", probe.record_errors);

    println!("\n== CELL ==");
    for (class, count) in &probe.cell_class {
        println!("  {count:>6}  {class}");
    }
    println!("block labels: {:?}", probe.block_labels);

    land_report(&probe);
    mismatch_report(&probe);
    quadrant_report(&probe);

    println!("\n== NAVM ==");
    println!("DATA sizes: {:?}", probe.nav_data_sizes);
    for (what, count) in &probe.nav {
        println!("  {count:>9}  {what}");
    }

    for (what, count) in &probe.nav_edges {
        println!("  {count:>9}  {what}");
    }
    let mut ext: Tally<String> = Tally::new();
    for (navmesh, triangle, unknown) in &probe.nav_external {
        let target = match probe.nav_triangles.get(navmesh) {
            Some(&count) if usize::from(*triangle) < count => "NAVM, triangle in range",
            Some(_) => "NAVM, triangle OUT of range",
            None => "not a NAVM in this file",
        };
        bump(&mut ext, format!("NVEX -> {target}"));
        bump(
            &mut ext,
            format!(
                "NVEX first 4 bytes {}",
                if *unknown == 0 { "zero" } else { "nonzero" }
            ),
        );
    }
    for door in &probe.nav_doors {
        bump(
            &mut ext,
            format!(
                "NVDP door -> {}",
                probe
                    .forms
                    .get(door)
                    .map_or("MISSING".to_owned(), |s| s.to_string())
            ),
        );
    }
    println!("NAVX {ext:?}");
    println!("misc: {:?}", probe.misc);
    println!("\n== CTDA ==\n{:?}", probe.ctda);
    println!("\n== ARMO DNAM ==\n{:?}", probe.armo);
    println!("\n== AMMO ==\n{:?}", probe.ammo);
    let kind = |form: FormId| {
        if form.is_null() {
            "null".to_owned()
        } else {
            probe
                .forms
                .get(&form)
                .map_or("MISSING".to_owned(), |s| s.to_string())
        }
    };
    let mut dat2: Tally<String> = Tally::new();
    for (projectile, consumed) in &probe.dat2_forms {
        bump(
            &mut dat2,
            format!(
                "projectile->{} consumed->{}",
                kind(*projectile),
                kind(*consumed)
            ),
        );
    }
    println!("DAT2 form targets: {dat2:?}");
    println!("\n== CREA DATA ==\n{:?}", probe.crea);
    println!("\n== XLOC ==\n{:?}", probe.xloc);
    let mut keys: Tally<String> = Tally::new();
    for key in &probe.xloc_keys {
        bump(&mut keys, kind(*key));
    }
    println!("XLOC key targets: {keys:?}");
    let mut xemi: Tally<String> = Tally::new();
    for form in &probe.xemi_forms {
        bump(&mut xemi, kind(*form));
    }
    println!("XEMI targets: {xemi:?}");
    println!("\n== REFR extras ==\n{:?}", probe.refr);

    println!("\n== Watched subrecord sizes ==");
    for ((record, sub, size), count) in &probe.sizes {
        println!("  {record}/{sub} size {size}: {count}");
    }
    println!("\n== Subrecords left in `unknown` of typed records (count >= 1) ==");
    let mut by_pair: BTreeMap<(String, String), (u64, Vec<usize>)> = BTreeMap::new();
    for ((record, sub, size), count) in &probe.unknown {
        let entry = by_pair.entry((record.clone(), sub.clone())).or_default();
        entry.0 += count;
        if entry.1.len() < 4 {
            entry.1.push(*size);
        }
    }
    for ((record, sub), (count, sizes)) in &by_pair {
        println!("  {record}/{sub}: {count} sizes {sizes:?}");
    }
}
