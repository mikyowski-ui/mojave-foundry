//! Real-data calibration of skeletons, skins, animations and posing.
//! Reads the archive named by `MOJAVE_MESHES_BSA` (and, when set, the one
//! named by `MOJAVE_UPDATE_BSA`, searched first) and prints tallies only:
//! counts, extents, distances, block type names. It writes nothing.
//!
//! ```text
//! MOJAVE_MESHES_BSA=/path/to/meshes.bsa cargo run -p nif --release --example characters
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use fnv_bsa::BsaArchive;
use nif::math::{Matrix34, apply_point, quat_to_rotation};
use nif::{
    Block, KEY_XYZ_ROTATION, NifFile, Pose, Skeleton, StaticMesh, extract_animation_with_report,
    extract_mesh, extract_skeleton, parse, parse_animation_file, pose, skin,
};

const SKELETON: &str = "meshes\\characters\\_male\\skeleton.nif";
const PARTS: [&str; 5] = [
    "meshes\\characters\\_male\\upperbody.nif",
    "meshes\\characters\\_male\\femaleupperbody.nif",
    "meshes\\characters\\_male\\lefthand.nif",
    "meshes\\characters\\_male\\righthand.nif",
    "meshes\\characters\\head\\headhuman.nif",
];
const IDLE: &str = "meshes\\characters\\_male\\locomotion\\mtidle.kf";
const WALK: &str = "meshes\\characters\\_male\\locomotion\\male\\mtforward.kf";

struct Archives(Vec<BsaArchive>);

impl Archives {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0
            .iter()
            .find_map(|a| a.find(path).and_then(|e| a.read(e).ok()))
    }
}

/// Hides numbers (and hexadecimal offsets) so that equal errors group.
fn mask(text: String) -> String {
    text.split(' ')
        .map(|word| {
            if word.starts_with("0x") {
                "#".to_owned()
            } else {
                word.chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Default)]
struct Tally(BTreeMap<String, u64>);

impl Tally {
    fn add(&mut self, key: impl Into<String>) {
        *self.0.entry(key.into()).or_default() += 1;
    }
    fn print(&self, title: &str) {
        println!("  {title}:");
        let mut ranked: Vec<_> = self.0.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (key, count) in ranked {
            println!("    {count:>7}  {key}");
        }
    }
}

fn bounds(mesh: &StaticMesh) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in mesh.primitives.iter().flat_map(|p| &p.positions) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// For each vertex of `a`, the distance to the nearest vertex of `b`:
/// the smallest and the median of those, over the 5 % of `a` nearest `b`.
fn seam(a: &StaticMesh, b: &StaticMesh) -> (f32, f32) {
    let bs: Vec<[f32; 3]> = b
        .primitives
        .iter()
        .flat_map(|p| p.positions.iter().copied())
        .collect();
    let mut nearest: Vec<f32> = a
        .primitives
        .iter()
        .flat_map(|p| &p.positions)
        .map(|&p| {
            bs.iter()
                .map(|&q| distance(p, q))
                .fold(f32::INFINITY, f32::min)
        })
        .collect();
    nearest.sort_by(f32::total_cmp);
    let band = (nearest.len() / 20).max(1);
    (
        nearest.first().copied().unwrap_or(f32::NAN),
        nearest.get(band / 2).copied().unwrap_or(f32::NAN),
    )
}

fn merge(meshes: &[StaticMesh]) -> StaticMesh {
    StaticMesh {
        primitives: meshes.iter().flat_map(|m| m.primitives.clone()).collect(),
        ..StaticMesh::default()
    }
}

fn print_box(label: &str, mesh: &StaticMesh) {
    let (lo, hi) = bounds(mesh);
    println!(
        "    {label}: x {:.1}..{:.1}  y {:.1}..{:.1}  z {:.1}..{:.1}  (height {:.1})",
        lo[0],
        hi[0],
        lo[1],
        hi[1],
        lo[2],
        hi[2],
        hi[2] - lo[2]
    );
}

fn skeleton_report(skeleton: &Skeleton) {
    println!("skeleton: {SKELETON}");
    println!(
        "  bones: {}  depth: {}  well formed: {}",
        skeleton.bones.len(),
        skeleton.depth(),
        skeleton.is_well_formed()
    );
    let roots = skeleton.bones.iter().filter(|b| b.parent.is_none()).count();
    println!("  roots: {roots}");
}

fn quaternion_convention(data: &[u8], file: &NifFile) {
    // NiTransformController: NiTimeController fields (next controller 4,
    // flags 2, frequency 4, phase 4, start 4, stop 4, target 4) then the
    // interpolator reference.
    let (mut same, mut transposed, mut neither, mut skipped) = (0, 0, 0, 0);
    for entry in &file.blocks {
        let Block::Node(node) = &entry.block else {
            continue;
        };
        let Some(controller) = node.av.net.controller else {
            continue;
        };
        let Some(raw) = file.blocks.get(controller as usize) else {
            continue;
        };
        if raw.type_name != "NiTransformController" || raw.size < 30 {
            continue;
        }
        let at = raw.offset + 26;
        let index = i32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let Some(Block::TransformInterpolator(interpolator)) = usize::try_from(index)
            .ok()
            .and_then(|i| file.blocks.get(i))
            .map(|e| &e.block)
        else {
            skipped += 1;
            continue;
        };
        let r = quat_to_rotation(interpolator.transform.rotation);
        let m = node.av.rotation;
        let diff = |f: &dyn Fn(usize, usize) -> f32| {
            (0..3)
                .flat_map(|i| (0..3).map(move |j| (i, j)))
                .map(|(i, j)| (f(i, j) - m[i][j]).abs())
                .fold(0.0f32, f32::max)
        };
        let symmetric = (0..3).all(|i| (0..3).all(|j| (m[i][j] - m[j][i]).abs() < 1e-3));
        if symmetric {
            skipped += 1;
            continue;
        }
        let d_same = diff(&|i, j| r[i][j]);
        let d_t = diff(&|i, j| r[j][i]);
        if d_same < 1e-3 {
            same += 1;
        } else if d_t < 1e-3 {
            transposed += 1;
        } else {
            neither += 1;
        }
    }
    println!(
        "  node rotation vs its interpolator's quaternion: equal {same}, transposed {transposed}, \
         neither {neither}, not informative {skipped}"
    );
}

struct Part {
    path: &'static str,
    mesh: StaticMesh,
}

fn skin_report(archives: &Archives, skeleton: &Skeleton) -> Vec<Part> {
    println!("\nskins:");
    let rest = Pose::rest(skeleton);
    let mut parts = Vec::new();
    for path in PARTS {
        let Some(data) = archives.read(path) else {
            println!("  {path}: not in the archive");
            continue;
        };
        let file = match parse(&data) {
            Ok(f) => f,
            Err(e) => {
                println!("  {path}: parse error: {}", mask(e.to_string()));
                continue;
            }
        };
        let mesh = match extract_mesh(&file) {
            Ok(m) => m,
            Err(e) => {
                println!("  {path}: extract error: {}", mask(e.to_string()));
                continue;
            }
        };
        let mut rows = 0usize;
        let mut bad_rows = 0usize;
        let mut bones = 0usize;
        let mut missing = Vec::new();
        let mut used_influences = [0usize; 5];
        for p in &mesh.primitives {
            let Some(s) = &p.skin else { continue };
            for row in &s.weights {
                rows += 1;
                let sum: f32 = row.iter().map(|&(_, w)| w).sum();
                if (sum - 1.0).abs() > 1e-4 {
                    bad_rows += 1;
                }
                used_influences[row.iter().filter(|&&(_, w)| w > 0.0).count()] += 1;
            }
            for b in &s.bones {
                bones += 1;
                if skeleton.find(&b.name).is_none() {
                    missing.push(b.name.clone());
                }
            }
        }
        missing.sort();
        missing.dedup();
        let mut stored_weights = Tally::default();
        let mut unweighted = 0usize;
        for e in &file.blocks {
            if let Block::SkinData(d) = &e.block {
                stored_weights.add(format!(
                    "skin data stores weights: {}",
                    d.has_vertex_weights
                ));
            }
            if let Some(shape) = e.block.as_shape()
                && let Some(instance) = shape.skin_instance
                && let Some(Block::TriShapeData(d)) = file.block(shape.data).map(|e| &e.block)
            {
                unweighted += nif::skin::unweighted_vertices(
                    &file,
                    instance,
                    usize::from(d.geom.num_vertices),
                )
                .unwrap_or(0);
            }
        }
        let skinned = mesh.primitives.iter().filter(|p| p.skin.is_some()).count();
        // Rest-pose skinning against the stored (bind) positions.
        let posed = skin(&mesh, skeleton, &rest);
        let mut worst = 0.0f32;
        for (a, b) in mesh.primitives.iter().zip(&posed.primitives) {
            for (p, q) in a.positions.iter().zip(&b.positions) {
                worst = worst.max(distance(*p, *q));
            }
        }
        println!(
            "  {path}: primitives {} (skinned {skinned}), weight rows {rows}, rows not summing \
             to 1: {bad_rows}, vertices without any stored weight: {unweighted}, skin bones \
             {bones}, not in the skeleton: {} {:?}",
            mesh.primitives.len(),
            missing.len(),
            missing
        );
        println!(
            "    influences per vertex (0..4): {used_influences:?}; rest-pose skinning moves a \
             vertex by at most {worst:.4} units"
        );
        stored_weights.print("weights source");
        parts.push(Part { path, mesh });
    }
    parts
}

fn kf_report(archives: &Archives, skeleton: &Skeleton) {
    println!("\nanimations under meshes\\characters\\_male\\:");
    let mut total = 0u64;
    let mut parsed = 0u64;
    let mut failures = Tally::default();
    let mut unhandled = Tally::default();
    let mut skipped = Tally::default();
    let mut keys = Tally::default();
    let mut cycles = Tally::default();
    let mut accum = Tally::default();
    let mut frequency = Tally::default();
    let mut versions = Tally::default();
    let mut versions_ok = Tally::default();
    let mut euler_folders = Tally::default();
    let mut euler = EulerCheck::default();
    let rest = skeleton.world_matrices();
    let _ = rest;
    let mut tracks = 0u64;
    let mut duplicates = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    for archive in &archives.0 {
        for entry in archive.entries() {
            if !(entry.path.starts_with("meshes\\characters\\_male\\")
                && entry.path.ends_with(".kf"))
                || !seen.insert(entry.path.clone())
            {
                continue;
            }
            total += 1;
            let Ok(data) = archive.read(entry) else {
                failures.add("archive read error");
                continue;
            };
            let file = match parse_animation_file(&data) {
                Ok(f) => {
                    versions_ok.add(format!("stream {}", f.header.stream_version));
                    f
                }
                Err(e) => {
                    if let nif::NifError::UnsupportedVersion {
                        version,
                        user_version,
                        stream_version,
                        ..
                    } = &e
                    {
                        versions.add(format!(
                            "{version} / user {user_version} / stream {stream_version}"
                        ));
                    }
                    failures.add(format!("parse: {}", mask(e.to_string())));
                    continue;
                }
            };
            for e in &file.blocks {
                if e.block.is_skipped() {
                    skipped.add(e.type_name.clone());
                }
                match &e.block {
                    Block::TransformData(d) => {
                        if d.rotation_type == KEY_XYZ_ROTATION {
                            let folder = entry.path.rsplit_once('\\').map_or("", |f| f.0);
                            euler_folders.add(folder.to_owned());
                            let groups = d.xyz_rotations.as_deref().unwrap();
                            for g in groups.iter().filter(|g| !g.keys.is_empty()) {
                                keys.add(format!("euler angle keys, type {}", g.interpolation));
                            }
                        } else if !d.quaternion_keys.is_empty() {
                            keys.add(format!("quaternion keys, type {}", d.rotation_type));
                        }
                        if !d.translations.keys.is_empty() {
                            keys.add(format!(
                                "translation keys, type {}",
                                d.translations.interpolation
                            ));
                        }
                        if !d.scales.keys.is_empty() {
                            keys.add(format!("scale keys, type {}", d.scales.interpolation));
                        }
                    }
                    Block::BSplineTransformInterpolator(s) => keys.add(format!(
                        "{} (compressed {})",
                        e.type_name,
                        s.compression.is_some()
                    )),
                    Block::ControllerSequence(s) => {
                        cycles.add(format!("cycle type {}", s.cycle_type));
                        accum.add(format!("accumulation root \"{}\"", s.accum_root_name));
                        frequency.add(format!("frequency {}", s.frequency));
                    }
                    _ => {}
                }
            }
            euler.check(&file, skeleton);
            match extract_animation_with_report(&file) {
                Ok((animation, report)) => {
                    parsed += 1;
                    tracks += animation.tracks.len() as u64;
                    duplicates += u64::from(report.duplicates);
                    for (name, n) in report.unhandled {
                        for _ in 0..n {
                            unhandled.add(name.clone());
                        }
                    }
                }
                Err(e) => failures.add(format!("animation: {}", mask(e.to_string()))),
            }
        }
    }
    println!(
        "  files {total}, parsed into an animation {parsed}, failed {}; tracks {tracks}, \
         duplicate bones {duplicates}",
        total - parsed
    );
    failures.print("failures");
    versions.print("versions of the files that failed on the version");
    versions_ok.print("stream versions of the files that parsed");
    euler_folders.print("folders with Euler angle rotation keys");
    euler.print();
    unhandled.print("controlled blocks left out, by interpolator type");
    skipped.print("blocks skipped by size");
    keys.print("key types");
    cycles.print("cycle types");
    accum.print("accumulation roots");
    frequency.print("frequencies");
}

/// Which Euler order puts Euler-keyed bones nearer the skeleton's rest
/// rotation at time 0, counted where the two orders differ by more than
/// two degrees.
#[derive(Default)]
struct EulerCheck {
    zyx_nearer: u64,
    xyz_nearer: u64,
    alike: u64,
    angle_zyx: f64,
    angle_xyz: f64,
}

impl EulerCheck {
    fn check(&mut self, file: &NifFile, skeleton: &Skeleton) {
        let Some(sequence) = file.blocks.iter().find_map(|e| match &e.block {
            Block::ControllerSequence(s) => Some(s),
            _ => None,
        }) else {
            return;
        };
        for block in &sequence.controlled_blocks {
            let Some(Block::TransformInterpolator(i)) =
                file.block(block.interpolator).map(|e| &e.block)
            else {
                continue;
            };
            let Some(Block::TransformData(d)) = file.block(i.data).map(|e| &e.block) else {
                continue;
            };
            let Some(groups) = d.xyz_rotations.as_deref() else {
                continue;
            };
            let Some(bone) = skeleton.find(&block.node_name) else {
                continue;
            };
            let a: [f32; 3] =
                std::array::from_fn(|k| groups[k].keys.first().map_or(0.0, |key| key.value));
            let zyx = nif::euler_xyz_to_quat(a[0], a[1], a[2]);
            let qx = nif::math::quat_axis_angle([1.0, 0.0, 0.0], a[0]);
            let qy = nif::math::quat_axis_angle([0.0, 1.0, 0.0], a[1]);
            let qz = nif::math::quat_axis_angle([0.0, 0.0, 1.0], a[2]);
            let xyz = nif::math::quat_mul(qx, nif::math::quat_mul(qy, qz));
            let rest = nif::math::rotation_to_quat(&skeleton.bones[bone as usize].local.rotation);
            let angle = |p: [f32; 4], q: [f32; 4]| -> f64 {
                let dot: f32 = p.iter().zip(&q).map(|(x, y)| x * y).sum();
                2.0 * f64::from(dot.abs().min(1.0)).acos().to_degrees()
            };
            let (dz, dx) = (angle(zyx, rest), angle(xyz, rest));
            self.angle_zyx += dz;
            self.angle_xyz += dx;
            if angle(zyx, xyz) < 2.0 {
                self.alike += 1;
            } else if dz < dx {
                self.zyx_nearer += 1;
            } else {
                self.xyz_nearer += 1;
            }
        }
    }

    fn print(&self) {
        let n = (self.zyx_nearer + self.xyz_nearer + self.alike).max(1) as f64;
        println!(
            "  Euler order at time 0 against the skeleton's rest rotation: X then Y then Z \
             (Rz·Ry·Rx) nearer {}, Z then Y then X (Rx·Ry·Rz) nearer {}, orders alike {}; mean \
             angle to rest {:.1}° and {:.1}°",
            self.zyx_nearer,
            self.xyz_nearer,
            self.alike,
            self.angle_zyx / n,
            self.angle_xyz / n
        );
    }
}

fn posing_report(archives: &Archives, skeleton: &Skeleton, parts: &[Part]) {
    println!("\nposing:");
    let meshes: Vec<StaticMesh> = parts
        .iter()
        .filter(|p| !p.path.contains("female"))
        .map(|p| p.mesh.clone())
        .collect();
    let hands: Vec<&Part> = parts.iter().filter(|p| p.path.contains("hand")).collect();
    let body = parts.iter().find(|p| p.path.ends_with("\\upperbody.nif"));
    let figure = |pose: &Pose| -> (StaticMesh, Vec<StaticMesh>, Option<StaticMesh>) {
        let posed: Vec<StaticMesh> = meshes.iter().map(|m| skin(m, skeleton, pose)).collect();
        let hand_meshes = hands
            .iter()
            .map(|h| skin(&h.mesh, skeleton, pose))
            .collect();
        let body = body.map(|b| skin(&b.mesh, skeleton, pose));
        (merge(&posed), hand_meshes, body)
    };
    let report = |label: &str, pose: &Pose| {
        let (all, hand_meshes, body) = figure(pose);
        print_box(label, &all);
        if let Some(body) = &body {
            for (h, part) in hand_meshes.iter().zip(&hands) {
                let (min, band) = seam(h, body);
                println!(
                    "      {} to the upper body: nearest vertex {min:.3}, median of the \
                     nearest 5 % {band:.3}",
                    part.path.rsplit('\\').next().unwrap_or("")
                );
            }
        }
    };
    let rest = Pose::rest(skeleton);
    report("skeleton rest pose", &rest);
    let stored = merge(&meshes);
    print_box("stored (bind) vertices", &stored);

    for path in [IDLE, WALK] {
        let Some(data) = archives.read(path) else {
            println!("  {path}: not in the archive");
            continue;
        };
        let animation = match nif::parse_animation(&data) {
            Ok(a) => a,
            Err(e) => {
                println!("  {path}: {}", mask(e.to_string()));
                continue;
            }
        };
        let named = animation
            .tracks
            .iter()
            .filter(|t| skeleton.find(&t.bone).is_some())
            .count();
        println!(
            "  {path}: duration {:.4} s, looping {}, tracks {} (bones found in the skeleton \
             {named}), accumulation root \"{}\"",
            animation.duration,
            animation.looping,
            animation.tracks.len(),
            animation.accum_root
        );
        // Which tracks move horizontally?
        for track in &animation.tracks {
            let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
            for (_, v) in &track.translations {
                for k in 0..2 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            let range = (hi[0] - lo[0]).max(hi[1] - lo[1]);
            if range > 5.0 {
                println!(
                    "    track \"{}\" moves {:.1} units horizontally ({} keys)",
                    track.bone,
                    range,
                    track.translations.len()
                );
            }
        }
        for fraction in [0.0f32, 0.25, 0.5, 0.75] {
            let t = animation.duration * fraction;
            let p = pose(skeleton, &animation, t);
            report(&format!("t = {t:.3} s"), &p);
            if fraction == 0.5 {
                println!(
                    "      travel at t: [{:.2}, {:.2}]",
                    p.travel[0], p.travel[1]
                );
            }
        }
        let end = pose(skeleton, &animation, animation.duration);
        let length = (end.travel[0].powi(2) + end.travel[1].powi(2)).sqrt();
        println!(
            "    travel over one cycle: [{:.2}, {:.2}] = {length:.2} units, {:.2} units per \
             second",
            end.travel[0],
            end.travel[1],
            if animation.duration > 0.0 {
                length / animation.duration
            } else {
                0.0
            }
        );
        let accum = skeleton.find(&animation.accum_root);
        if let Some(i) = accum {
            let w: &Matrix34 = &end.world[i as usize];
            let z = apply_point(w, [0.0; 3]);
            println!(
                "    accumulation root at the cycle's end, held in place: ({:.2}, {:.2}, {:.2})",
                z[0], z[1], z[2]
            );
        }
    }
}

fn corpus_report(archives: &Archives) {
    println!("\nmesh extraction over every .nif:");
    let mut files = 0u64;
    let mut extracted = 0u64;
    let mut skinned_files = 0u64;
    let mut skins = 0u64;
    let mut errors = Tally::default();
    let mut seen = std::collections::BTreeSet::new();
    for archive in &archives.0 {
        for entry in archive.entries() {
            if !entry.path.ends_with(".nif") || !seen.insert(entry.path.clone()) {
                continue;
            }
            files += 1;
            let Ok(data) = archive.read(entry) else {
                continue;
            };
            let Ok(file) = parse(&data) else { continue };
            match extract_mesh(&file) {
                Ok(mesh) => {
                    extracted += 1;
                    let n = mesh.primitives.iter().filter(|p| p.skin.is_some()).count() as u64;
                    skins += n;
                    skinned_files += u64::from(n > 0);
                    if !mesh
                        .primitives
                        .iter()
                        .all(|p| p.skinned == p.skin.is_some())
                    {
                        errors.add("skinned flag disagrees with the skin");
                    }
                }
                Err(e) => errors.add(mask(e.to_string())),
            }
        }
    }
    println!(
        "  files {files}, extracted {extracted}, files with a skinned primitive {skinned_files}, \
         skinned primitives {skins}"
    );
    errors.print("extraction errors");
}

fn main() {
    let mut archives = Vec::new();
    for var in ["MOJAVE_UPDATE_BSA", "MOJAVE_MESHES_BSA"] {
        if let Some(path) = std::env::var_os(var) {
            archives.push(BsaArchive::open(Path::new(&path)).expect("open archive"));
        }
    }
    if archives.is_empty() {
        eprintln!("set MOJAVE_MESHES_BSA to the meshes archive");
        std::process::exit(2);
    }
    let archives = Archives(archives);
    let Some(data) = archives.read(SKELETON) else {
        eprintln!("{SKELETON} is not in the archive");
        std::process::exit(1);
    };
    let file = parse(&data).expect("skeleton parses");
    let skeleton = extract_skeleton(&file).expect("skeleton extracts");
    skeleton_report(&skeleton);
    quaternion_convention(&data, &file);
    let parts = skin_report(&archives, &skeleton);
    posing_report(&archives, &skeleton, &parts);
    kf_report(&archives, &skeleton);
    if std::env::var_os("MOJAVE_CORPUS").is_some() {
        corpus_report(&archives);
    }
}
