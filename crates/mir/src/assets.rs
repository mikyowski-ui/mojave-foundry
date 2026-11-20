//! Converting the slice's meshes to `.glb`, textures to `.png`, and the
//! actors' skeletons and animations to `.json` documents, each named by
//! the content hash of its source bytes.
//!
//! Paths are worked on by `jobs` scoped threads. Each thread reads and
//! hashes a path, then claims its hash: the first claimer of a hash
//! converts it (or reuses the file a previous run wrote), the others only
//! record the hash. Results are collected by path, so nothing written
//! depends on which thread did what.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::error::MirError;
use crate::finite;
use crate::index::content_hash;
use crate::model::{
    AnimationInfo, AssetEntry, AssetStatus, Image, MeshInfo, SkeletonInfo, TextureInfo,
};
use crate::paths::normalize_texture_path;
use crate::source::DataSource;

/// Counts for one kind of asset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssetCounts {
    /// Distinct source paths the slice uses.
    pub referenced: usize,
    /// Distinct contents converted by this run.
    pub converted: usize,
    /// Distinct contents whose output an earlier run had written.
    pub reused: usize,
    /// Paths that are missing, unreadable or undecodable.
    pub failed: usize,
}

/// What happened to one hash.
enum HashOutcome<I> {
    Converted(I),
    Reused(I),
    Failed(String),
}

/// What happened to one path.
enum PathOutcome<I> {
    Missing,
    Unreadable(String),
    /// The path's bytes have this hash; `Some` when this path claimed it.
    Hashed(String, Option<HashOutcome<I>>),
}

/// A path's hash, or why it has none.
type PathResult = Result<String, (AssetStatus, String)>;

/// The result of converting one kind of asset.
pub(crate) struct Converted<I> {
    pub entries: BTreeMap<String, AssetEntry>,
    pub info: BTreeMap<String, I>,
    pub counts: AssetCounts,
}

/// Writes `bytes` to `path` through a temporary file beside it.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MirError> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes).map_err(|e| MirError::io(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| MirError::io(path, e))
}

/// Runs `convert` for every path on `jobs` threads.
fn run<I: Send>(
    source: &DataSource,
    paths: &[String],
    jobs: usize,
    convert: &(dyn Fn(&str, &[u8]) -> HashOutcome<I> + Sync),
) -> Converted<I> {
    let claimed: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    let next = AtomicUsize::new(0);
    let worker = || {
        let mut done: Vec<(usize, PathOutcome<I>)> = Vec::new();
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(path) = paths.get(i) else {
                break;
            };
            let outcome = match source.read(path) {
                Ok(None) => PathOutcome::Missing,
                Err(error) => PathOutcome::Unreadable(error.to_string()),
                Ok(Some(bytes)) => {
                    let hash = content_hash(&bytes);
                    let first = claimed
                        .lock()
                        .map(|mut set| set.insert(hash.clone()))
                        .unwrap_or(false);
                    let result = first.then(|| convert(&hash, &bytes));
                    PathOutcome::Hashed(hash, result)
                }
            };
            done.push((i, outcome));
        }
        done
    };
    let jobs = jobs.clamp(1, paths.len().max(1));
    let mut outcomes: Vec<(usize, PathOutcome<I>)> = if jobs == 1 {
        worker()
    } else {
        thread::scope(|scope| {
            let handles: Vec<_> = (0..jobs).map(|_| scope.spawn(worker)).collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    };
    outcomes.sort_by_key(|(i, _)| *i);

    let mut counts = AssetCounts {
        referenced: paths.len(),
        ..AssetCounts::default()
    };
    let mut info = BTreeMap::new();
    let mut failures: BTreeMap<String, String> = BTreeMap::new();
    let mut hashes: Vec<(usize, PathResult)> = Vec::new();
    for (i, outcome) in outcomes {
        let result = match outcome {
            PathOutcome::Missing => Err((
                AssetStatus::Missing,
                "no loose file and no archive has it".to_owned(),
            )),
            PathOutcome::Unreadable(reason) => Err((AssetStatus::Unreadable, reason)),
            PathOutcome::Hashed(hash, outcome) => {
                match outcome {
                    Some(HashOutcome::Converted(i)) => {
                        counts.converted += 1;
                        info.insert(hash.clone(), i);
                    }
                    Some(HashOutcome::Reused(i)) => {
                        counts.reused += 1;
                        info.insert(hash.clone(), i);
                    }
                    Some(HashOutcome::Failed(reason)) => {
                        failures.insert(hash.clone(), reason);
                    }
                    None => {}
                }
                Ok(hash)
            }
        };
        hashes.push((i, result));
    }
    let mut entries = BTreeMap::new();
    for (i, result) in hashes {
        let entry = match result {
            Ok(hash) if info.contains_key(&hash) => AssetEntry {
                status: AssetStatus::Converted,
                hash: Some(hash),
                reason: None,
            },
            Ok(hash) => AssetEntry {
                status: AssetStatus::Undecodable,
                reason: Some(
                    failures
                        .get(&hash)
                        .cloned()
                        .unwrap_or_else(|| "conversion failed".to_owned()),
                ),
                hash: Some(hash),
            },
            Err((status, reason)) => AssetEntry {
                status,
                hash: None,
                reason: Some(reason),
            },
        };
        if entry.status != AssetStatus::Converted {
            counts.failed += 1;
        }
        entries.insert(paths[i].clone(), entry);
    }
    Converted {
        entries,
        info,
        counts,
    }
}

/// Facts about a mesh, as the manifest keeps them.
pub(crate) fn mesh_info(mesh: &nif::StaticMesh) -> MeshInfo {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut vertices = 0u64;
    let mut triangles = 0u64;
    let mut textures = BTreeSet::new();
    let mut skinned = 0u32;
    for primitive in &mesh.primitives {
        vertices += primitive.positions.len() as u64;
        triangles += primitive.indices.len() as u64 / 3;
        skinned += u32::from(primitive.skinned);
        for p in primitive
            .positions
            .iter()
            .filter(|p| p.iter().all(|v| v.is_finite()))
        {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        for texture in &primitive.material.textures {
            let path = normalize_texture_path(texture);
            if !path.is_empty() {
                textures.insert(path);
            }
        }
    }
    let clamp = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    MeshInfo {
        primitives: clamp(mesh.primitives.len() as u64),
        vertices: clamp(vertices),
        triangles: clamp(triangles),
        bounds: min[0].is_finite().then_some([min, max]),
        textures: textures.into_iter().collect(),
        skipped_blocks: mesh.skipped_blocks.clone(),
        collision: mesh.collision.is_some(),
        skinned_primitives: skinned,
    }
}

fn convert_mesh(dir: &Path, hash: &str, bytes: &[u8]) -> HashOutcome<MeshInfo> {
    let target = dir.join(format!("{hash}.glb"));
    if let Ok(existing) = fs::read(&target)
        && let Ok(mesh) = nif::glb::read(&existing)
    {
        return HashOutcome::Reused(mesh_info(&mesh));
    }
    let glb = match nif::parse(bytes).and_then(|file| nif::extract_mesh(&file)) {
        Ok(mesh) => nif::glb::write(&mesh),
        Err(error) => return HashOutcome::Failed(error.to_string()),
    };
    // The manifest describes the file as it reads back.
    let mesh = match nif::glb::read(&glb) {
        Ok(mesh) => mesh,
        Err(error) => {
            return HashOutcome::Failed(format!("written .glb does not read back: {error}"));
        }
    };
    match write_atomic(&target, &glb) {
        Ok(()) => HashOutcome::Converted(mesh_info(&mesh)),
        Err(error) => HashOutcome::Failed(error.to_string()),
    }
}

/// Encodes RGBA8 as PNG with fixed settings.
pub(crate) fn encode_png(image: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::Paeth);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&image.rgba)
            .map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Decodes a PNG to RGBA8. Only what [`encode_png`] writes, and other
/// 8-bit RGBA, RGB, grey and grey-alpha files, are accepted.
pub(crate) fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| "image too large".to_owned())?;
    let mut buffer = vec![0u8; size];
    let info = reader.next_frame(&mut buffer).map_err(|e| e.to_string())?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("bit depth {:?}, expected 8", info.bit_depth));
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let pixels = w * h;
    let mut rgba = Vec::with_capacity(pixels * 4);
    let line = info.line_size;
    for row in 0..h {
        let data = buffer
            .get(row * line..row * line + line)
            .ok_or_else(|| "short image data".to_owned())?;
        match info.color_type {
            png::ColorType::Rgba => rgba.extend_from_slice(&data[..w * 4]),
            png::ColorType::Rgb => {
                for px in data[..w * 3].as_chunks::<3>().0 {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for &g in &data[..w] {
                    rgba.extend_from_slice(&[g, g, g, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for px in data[..w * 2].as_chunks::<2>().0 {
                    rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
                }
            }
            other => return Err(format!("colour type {other:?} is not supported")),
        }
    }
    Ok(Image {
        width: info.width,
        height: info.height,
        rgba,
    })
}

fn png_size(bytes: &[u8]) -> Option<TextureInfo> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let reader = decoder.read_info().ok()?;
    let info = reader.info();
    Some(TextureInfo {
        width: info.width,
        height: info.height,
    })
}

fn convert_texture(dir: &Path, hash: &str, bytes: &[u8]) -> HashOutcome<TextureInfo> {
    let target = dir.join(format!("{hash}.png"));
    if let Ok(existing) = fs::read(&target)
        && let Some(info) = png_size(&existing)
    {
        return HashOutcome::Reused(info);
    }
    let image = match dds::decode(bytes) {
        Ok(image) => Image {
            width: image.width,
            height: image.height,
            rgba: image.rgba,
        },
        Err(error) => return HashOutcome::Failed(error.to_string()),
    };
    let png = match encode_png(&image) {
        Ok(png) => png,
        Err(error) => return HashOutcome::Failed(format!("PNG encoding failed: {error}")),
    };
    match write_atomic(&target, &png) {
        Ok(()) => HashOutcome::Converted(TextureInfo {
            width: image.width,
            height: image.height,
        }),
        Err(error) => HashOutcome::Failed(error.to_string()),
    }
}

/// Facts about a skeleton, as the manifest keeps them; `paths` is filled
/// in by [`skeletons`].
pub(crate) fn skeleton_info(skeleton: &nif::Skeleton) -> SkeletonInfo {
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    SkeletonInfo {
        paths: Vec::new(),
        bones: clamp(skeleton.bones.len()),
        depth: clamp(skeleton.depth()),
        roots: clamp(skeleton.bones.iter().filter(|b| b.parent.is_none()).count()),
    }
}

/// Facts about an animation, as the manifest keeps them; `paths` is filled
/// in by [`animations`].
pub(crate) fn animation_info(animation: &nif::Animation) -> AnimationInfo {
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    AnimationInfo {
        paths: Vec::new(),
        name: animation.name.clone(),
        duration: animation.duration,
        looping: animation.looping,
        tracks: clamp(animation.tracks.len()),
        text_keys: clamp(animation.text_keys.len()),
        accum_root: animation.accum_root.clone(),
    }
}

/// Why a skeleton cannot be stored as JSON, if it cannot.
fn skeleton_problem(skeleton: &nif::Skeleton) -> Option<String> {
    if skeleton.bones.is_empty() {
        return Some("the file has no nodes".to_owned());
    }
    if !skeleton.is_well_formed() {
        return Some("a bone's parent does not precede it".to_owned());
    }
    skeleton
        .bones
        .iter()
        .find(|b| {
            let t = &b.local;
            !(finite::all(t.rotation.as_flattened())
                && finite::all(&t.translation)
                && t.scale.is_finite())
        })
        .map(|b| format!("bone {:?} has a non-finite transform", b.name))
}

/// Why an animation cannot be stored as JSON, if it cannot.
fn animation_problem(animation: &nif::Animation) -> Option<String> {
    if !animation.duration.is_finite() {
        return Some("the duration is not finite".to_owned());
    }
    if animation.text_keys.iter().any(|(t, _)| !t.is_finite()) {
        return Some("a text key time is not finite".to_owned());
    }
    animation
        .tracks
        .iter()
        .find(|t| {
            !(t.rotations
                .iter()
                .all(|(time, q)| time.is_finite() && finite::all(q))
                && t.translations
                    .iter()
                    .all(|(time, v)| time.is_finite() && finite::all(v))
                && t.scales
                    .iter()
                    .all(|(time, s)| time.is_finite() && s.is_finite()))
        })
        .map(|t| format!("the track of bone {:?} has a non-finite key", t.bone))
}

/// Stores a document as `<dir>/<hash>.json`: reuses a file an earlier run
/// wrote when it parses, otherwise converts `bytes` with `convert`, checks
/// the result, writes it and describes the document as it reads back.
fn convert_document<T, I>(
    dir: &Path,
    hash: &str,
    bytes: &[u8],
    convert: impl Fn(&[u8]) -> Result<T, nif::NifError>,
    problem: impl Fn(&T) -> Option<String>,
    info: impl Fn(&T) -> I,
) -> HashOutcome<I>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let target = dir.join(format!("{hash}.json"));
    if let Ok(existing) = fs::read(&target)
        && let Ok(document) = serde_json::from_slice::<T>(&existing)
        && problem(&document).is_none()
    {
        return HashOutcome::Reused(info(&document));
    }
    let document = match convert(bytes) {
        Ok(document) => document,
        Err(error) => return HashOutcome::Failed(error.to_string()),
    };
    if let Some(reason) = problem(&document) {
        return HashOutcome::Failed(reason);
    }
    let mut json = match serde_json::to_vec(&document) {
        Ok(json) => json,
        Err(error) => return HashOutcome::Failed(format!("JSON encoding failed: {error}")),
    };
    json.push(b'\n');
    let back: T = match serde_json::from_slice(&json) {
        Ok(back) => back,
        Err(error) => {
            return HashOutcome::Failed(format!("written JSON does not read back: {error}"));
        }
    };
    match write_atomic(&target, &json) {
        Ok(()) => HashOutcome::Converted(info(&back)),
        Err(error) => HashOutcome::Failed(error.to_string()),
    }
}

/// Converts every skeleton path (normalised, sorted, distinct) into `dir`
/// as `<hash>.json`, with `nif::parse` and `nif::extract_skeleton`.
pub(crate) fn skeletons(
    source: &DataSource,
    paths: &[String],
    dir: &Path,
    jobs: usize,
) -> Converted<SkeletonInfo> {
    let mut converted = run(source, paths, jobs, &|hash, bytes| {
        convert_document(
            dir,
            hash,
            bytes,
            |b| nif::parse(b).and_then(|file| nif::extract_skeleton(&file)),
            skeleton_problem,
            skeleton_info,
        )
    });
    for (path, entry) in &converted.entries {
        if let Some(info) = entry
            .converted_hash()
            .and_then(|h| converted.info.get_mut(h))
        {
            info.paths.push(path.clone());
        }
    }
    converted
}

/// Converts every animation path (normalised, sorted, distinct) into `dir`
/// as `<hash>.json`, with `nif::parse_animation`.
pub(crate) fn animations(
    source: &DataSource,
    paths: &[String],
    dir: &Path,
    jobs: usize,
) -> Converted<AnimationInfo> {
    let mut converted = run(source, paths, jobs, &|hash, bytes| {
        convert_document(
            dir,
            hash,
            bytes,
            nif::parse_animation,
            animation_problem,
            animation_info,
        )
    });
    for (path, entry) in &converted.entries {
        if let Some(info) = entry
            .converted_hash()
            .and_then(|h| converted.info.get_mut(h))
        {
            info.paths.push(path.clone());
        }
    }
    converted
}

/// Converts every mesh path (normalised, sorted, distinct) into `dir`.
pub(crate) fn meshes(
    source: &DataSource,
    paths: &[String],
    dir: &Path,
    jobs: usize,
) -> Converted<MeshInfo> {
    run(source, paths, jobs, &|hash, bytes| {
        convert_mesh(dir, hash, bytes)
    })
}

/// Converts every texture path (normalised, sorted, distinct) into `dir`.
pub(crate) fn textures(
    source: &DataSource,
    paths: &[String],
    dir: &Path,
    jobs: usize,
) -> Converted<TextureInfo> {
    run(source, paths, jobs, &|hash, bytes| {
        convert_texture(dir, hash, bytes)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_is_exact_and_stable() {
        let rgba: Vec<u8> = (0..(5 * 3 * 4)).map(|i| (i * 37 % 256) as u8).collect();
        let image = Image {
            width: 5,
            height: 3,
            rgba,
        };
        let a = encode_png(&image).unwrap();
        let b = encode_png(&image).unwrap();
        assert_eq!(a, b);
        assert_eq!(decode_png(&a).unwrap(), image);
        assert_eq!(
            png_size(&a),
            Some(TextureInfo {
                width: 5,
                height: 3
            })
        );
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn skeletons_and_animations_that_json_cannot_hold_are_refused() {
        let file = nif::parse(&nif::fixture::skeleton_nif()).unwrap();
        let mut skeleton = nif::extract_skeleton(&file).unwrap();
        assert_eq!(skeleton_problem(&skeleton), None);
        let info = skeleton_info(&skeleton);
        assert_eq!((info.bones, info.depth, info.roots), (6, 5, 1));
        assert!(skeleton_problem(&nif::Skeleton::default()).is_some());
        skeleton.bones[3].local.translation[1] = f32::NAN;
        assert!(
            skeleton_problem(&skeleton)
                .unwrap()
                .contains("\"Bip01 Spine\"")
        );
        skeleton.bones[3].local.translation[1] = 0.0;
        skeleton.bones[1].parent = Some(4);
        assert!(skeleton_problem(&skeleton).is_some());

        let mut animation = nif::parse_animation(&nif::fixture::walk_kf()).unwrap();
        assert_eq!(animation_problem(&animation), None);
        let info = animation_info(&animation);
        assert_eq!(info.duration, nif::fixture::WALK_DURATION);
        assert!(info.looping && info.paths.is_empty());
        animation.tracks[0].translations[0].1[2] = f32::INFINITY;
        assert!(animation_problem(&animation).unwrap().contains("\"Bip01\""));
        animation.tracks[0].translations[0].1[2] = 0.0;
        animation.duration = f32::NAN;
        assert!(animation_problem(&animation).is_some());
    }

    #[test]
    fn a_document_is_reused_converted_or_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = nif::fixture::skeleton_nif();
        let hash = content_hash(&bytes);
        let convert = |b: &[u8]| nif::parse(b).and_then(|f| nif::extract_skeleton(&f));
        let first = convert_document(
            dir.path(),
            &hash,
            &bytes,
            convert,
            skeleton_problem,
            skeleton_info,
        );
        assert!(matches!(first, HashOutcome::Converted(ref i) if i.bones == 6));
        let again = convert_document(
            dir.path(),
            &hash,
            &bytes,
            convert,
            skeleton_problem,
            skeleton_info,
        );
        assert!(matches!(again, HashOutcome::Reused(_)));
        // A damaged earlier file is converted again.
        fs::write(dir.path().join(format!("{hash}.json")), b"{").unwrap();
        let fixed = convert_document(
            dir.path(),
            &hash,
            &bytes,
            convert,
            skeleton_problem,
            skeleton_info,
        );
        assert!(matches!(fixed, HashOutcome::Converted(_)));
        let bad = convert_document(
            dir.path(),
            "x",
            b"garbage",
            convert,
            skeleton_problem,
            skeleton_info,
        );
        assert!(matches!(bad, HashOutcome::Failed(_)));
        assert!(!dir.path().join("x.json").exists());
    }
}
