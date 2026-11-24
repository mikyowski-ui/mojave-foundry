//! The sprite stage: every distinct mesh the slice places is rendered once,
//! quantized to the Fallout 2 palette and written as a single-frame FRM.
//!
//! Output, under `<out>/sprites/`:
//!
//! - `<key>.frm` and `<key>.json`, where `<key>` is the content hash of
//!   everything that determines the sprite (mesh, textures, calibration,
//!   palette, render path, options). An existing pair is reused untouched,
//!   which is the incremental cache.
//! - `index.json`: one entry per mesh, sorted by mesh hash.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use foundry_render::{Backend, Instance, Scene, Texture, TextureSource, Window};
use mir::Mir;
use quantize::{Dither, PaletteMap};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vault_formats::frm::FrmArchive;

use crate::config::{ConvertError, Inputs};

/// Bumped whenever the stage's output for the same inputs would change.
const STAGE_VERSION: &str = "sprites-1";

/// Reference kinds that are placed actors, not props: their meshes are
/// skinned and belong to a later milestone.
fn is_prop(reference: &mir::Reference) -> bool {
    !reference.flags.deleted
        && !matches!(
            reference.base_kind.as_deref(),
            Some("NPC_" | "CREA" | "LVLN" | "LVLC")
        )
}

/// What `<key>.json` records about one sprite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpriteMeta {
    pub mesh: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
    /// The FRM direction shift that places the frame where it was rendered
    /// when the object's anchor is the hex centre.
    pub shift: [i16; 2],
    pub triangles: u32,
    pub opaque_pixels: u64,
    pub colours: usize,
    pub mean_error: f64,
    pub max_error: f64,
    pub missing_textures: Vec<String>,
    pub backend: String,
}

/// One row of `index.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpriteEntry {
    pub mesh: String,
    pub model: String,
    /// How many references in the slice use this mesh.
    pub references: usize,
    pub base_kinds: Vec<String>,
    /// The cache key; `<key>.frm` exists when `error` is `None`.
    pub key: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpritesSummary {
    pub meshes: usize,
    pub rendered: usize,
    pub reused: usize,
    pub failed: usize,
}

/// Resolves a material's texture path through the MIR directory, decoding
/// each texture once per worker.
pub(crate) struct MirTextures<'a> {
    pub(crate) mir: &'a Mir,
    pub(crate) cache: RefCell<BTreeMap<String, Option<Texture>>>,
}

impl TextureSource for MirTextures<'_> {
    fn texture(&self, path: &str) -> Option<Cow<'_, Texture>> {
        let mut cache = self.cache.borrow_mut();
        let slot = cache.entry(path.to_owned()).or_insert_with(|| {
            let hash = self.mir.texture_hash(path)?;
            let image = self.mir.texture(hash).ok()?;
            Some(Texture {
                width: image.width,
                height: image.height,
                rgba: image.rgba,
            })
        });
        slot.clone().map(Cow::Owned)
    }
}

/// The cache key with whether the sprite was rendered now, or the key (empty
/// when the mesh itself could not be read) with the reason it failed.
type Built = Result<(String, bool), (String, String)>;

struct Job {
    mesh: String,
    model: String,
    references: usize,
    base_kinds: BTreeSet<String>,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ConvertError + '_ {
    move |source| ConvertError::Io {
        path: path.to_owned(),
        source,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Cpu => "cpu",
        Backend::Gpu => "gpu",
        Backend::Auto => "auto",
    }
}

/// Every distinct mesh the slice's exterior and interior cells place.
fn collect(mir: &Mir) -> Result<Vec<Job>, ConvertError> {
    let stage = |e: mir::MirError| ConvertError::stage("sprites", e);
    let mut jobs: BTreeMap<String, Job> = BTreeMap::new();
    let mut add = |doc: &mir::CellDoc| {
        for reference in doc.references.iter().filter(|r| is_prop(r)) {
            let Some(mesh) = &reference.mesh else {
                continue;
            };
            let job = jobs.entry(mesh.clone()).or_insert_with(|| Job {
                mesh: mesh.clone(),
                model: reference.model.clone().unwrap_or_default(),
                references: 0,
                base_kinds: BTreeSet::new(),
            });
            job.references += 1;
            if let Some(kind) = &reference.base_kind {
                job.base_kinds.insert(kind.clone());
            }
        }
    };
    for key in mir.cells() {
        if let Some(doc) = mir.cell(&key.worldspace, key.x, key.y).map_err(stage)? {
            add(&doc);
        }
    }
    Ok(jobs.into_values().collect())
}

struct Shared<'a> {
    mir: &'a Mir,
    inputs: &'a Inputs,
    map: &'a PaletteMap,
    /// Everything but the mesh and its textures, hashed once.
    key_prefix: Vec<u8>,
    dir: PathBuf,
}

impl Shared<'_> {
    fn key(&self, job: &Job, mesh: &nif::StaticMesh) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.key_prefix);
        hasher.update(job.mesh.as_bytes());
        // A texture that changes changes the sprite, even when the mesh
        // does not.
        for path in mesh.texture_paths() {
            hasher.update(path.as_bytes());
            hasher.update([0]);
            hasher.update(self.mir.texture_hash(&path).unwrap_or("missing").as_bytes());
            hasher.update([0]);
        }
        hex(&hasher.finalize())
    }

    /// Renders, quantizes and writes one sprite. `Ok(true)` when it was
    /// rendered, `Ok(false)` when the cache already had it.
    fn build(
        &self,
        job: &Job,
        renderer: &mut dyn foundry_render::Renderer,
        textures: &MirTextures<'_>,
    ) -> Built {
        let mesh = self
            .mir
            .mesh(&job.mesh)
            .map_err(|e| (String::new(), format!("mesh: {e}")))?;
        let key = self.key(job, &mesh);
        let fail = |reason: String| (key.clone(), reason);
        let frm_path = self.dir.join(format!("{key}.frm"));
        let meta_path = self.dir.join(format!("{key}.json"));
        if frm_path.is_file() && meta_path.is_file() {
            return Ok((key, false));
        }
        if mesh.primitives.is_empty() {
            return Err(fail("the mesh has no geometry".to_owned()));
        }
        let scene = Scene {
            anchor: [0.0; 3],
            instances: vec![Instance::new(&mesh, Instance::IDENTITY, textures)],
        };
        let output = renderer
            .render(&scene, Window::Fit)
            .map_err(|e| fail(format!("render: {e}")))?;
        let mut indexed = quantize::quantize(
            &output.rgba,
            output.width,
            output.height,
            self.map,
            Dither::None,
        )
        .map_err(|e| fail(format!("quantize: {e}")))?;
        let stats = quantize::stats(
            &output.rgba,
            &indexed,
            output.width,
            output.height,
            self.map,
        )
        .map_err(|e| fail(format!("quantize: {e}")))?;
        if stats.opaque_pixels == 0 {
            return Err(fail("the render is empty".to_owned()));
        }
        if self.inputs.outline {
            quantize::outline(&mut indexed, output.width, output.height, self.map)
                .map_err(|e| fail(format!("outline: {e}")))?;
        }
        let placed = emit::placed_frame(&indexed, output.width, output.height, output.origin)
            .map_err(|e| fail(format!("frame: {e}")))?;
        let meta = SpriteMeta {
            mesh: job.mesh.clone(),
            model: job.model.clone(),
            width: u32::from(placed.frame.width),
            height: u32::from(placed.frame.height),
            shift: placed.shift,
            triangles: output.triangles,
            opaque_pixels: stats.opaque_pixels,
            colours: stats.used_indices().len(),
            mean_error: stats.mean_error,
            max_error: stats.max_error,
            missing_textures: output.missing_textures.clone(),
            backend: backend_name(output.backend).to_owned(),
        };
        let spec = emit::FrmSpec {
            fps: 0,
            action_frame: 0,
            directions: vec![emit::FrmDirection {
                shift: placed.shift,
                frames: vec![placed.frame],
            }],
        };
        let bytes = emit::FormatWriters::frm(&emit::LocalWriters, &spec)
            .map_err(|e| fail(format!("frm: {e}")))?;
        // Never leave a file the engine's own parser would refuse.
        FrmArchive::parse(&bytes).map_err(|e| fail(format!("frm does not parse back: {e}")))?;
        let meta_text = serde_json::to_string_pretty(&meta).expect("plain data");
        std::fs::write(&frm_path, bytes).map_err(|e| fail(format!("write: {e}")))?;
        std::fs::write(&meta_path, meta_text + "\n").map_err(|e| fail(format!("write: {e}")))?;
        Ok((key, true))
    }
}

/// Everything but the mesh and its textures that a sprite's cache key
/// hashes: the stage version, calibration, palette, render path, outline
/// and (from Fallout 3 on) the game. The map stage's item sprites
/// (`crate::catalogue`) key their files in `<out>/sprites/` the same way.
pub(crate) fn key_prefix(
    inputs: &Inputs,
    backend: Backend,
    palette_bytes: &[u8],
) -> Result<Vec<u8>, ConvertError> {
    let mut prefix = Sha256::new();
    prefix.update(STAGE_VERSION.as_bytes());
    prefix.update(
        inputs
            .calibration
            .to_toml()
            .map_err(|e| ConvertError::stage("sprites", e))?
            .as_bytes(),
    );
    prefix.update(Sha256::digest(palette_bytes));
    prefix.update(backend_name(backend).as_bytes());
    prefix.update([u8::from(inputs.outline)]);
    // The game is in the key from Fallout 3 on; a New Vegas run hashes
    // nothing new, so its existing sprites stay valid.
    if inputs.game != mir::GameVariant::FalloutNewVegas {
        prefix.update(inputs.game.id().as_bytes());
    }
    Ok(prefix.finalize().to_vec())
}

/// Renders every prop mesh of the slice to `<out>/sprites/`.
pub fn sprites(inputs: &Inputs) -> Result<SpritesSummary, ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("sprites", e);
    let mir = Mir::open(&inputs.mir_dir()).map_err(|e| stage(&e))?;
    let palette_bytes = inputs.f2_file("color.pal")?;
    let palette = inputs.palette()?;
    let map = PaletteMap::try_new(&palette).map_err(|e| stage(&e))?;
    let dir = inputs.sprites_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;

    // The render path is part of the key: the two paths differ in a few
    // edge pixels, so their outputs must never share a cache entry.
    let backend = match inputs.backend {
        Backend::Auto => foundry_render::renderer(Backend::Auto, &inputs.calibration)
            .map_err(|e| stage(&e))?
            .backend(),
        other => other,
    };
    let shared = Shared {
        mir: &mir,
        inputs,
        map: &map,
        key_prefix: key_prefix(inputs, backend, &palette_bytes)?,
        dir: dir.clone(),
    };

    let jobs = collect(&mir)?;
    // One GPU device serves the whole run; the CPU path gets a renderer per
    // worker. Either way results are gathered in job order.
    let workers = if backend == Backend::Gpu {
        1
    } else {
        inputs.jobs.clamp(1, jobs.len().max(1))
    };
    let chunk = jobs.len().div_ceil(workers).max(1);
    let run = |slice: &[Job]| -> Result<Vec<Built>, String> {
        let mut renderer =
            foundry_render::renderer(backend, &inputs.calibration).map_err(|e| e.to_string())?;
        let textures = MirTextures {
            mir: &mir,
            cache: RefCell::new(BTreeMap::new()),
        };
        Ok(slice
            .iter()
            .map(|job| shared.build(job, renderer.as_mut(), &textures))
            .collect())
    };
    let results: Vec<Built> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|slice| scope.spawn(move || run(slice)))
            .collect();
        let mut all = Vec::with_capacity(jobs.len());
        for handle in handles {
            let part = handle
                .join()
                .map_err(|_| "a render worker panicked".to_owned())
                .and_then(|r| r)
                .map_err(|e| stage(&e))?;
            all.extend(part);
        }
        Ok::<_, ConvertError>(all)
    })?;

    let mut summary = SpritesSummary {
        meshes: jobs.len(),
        ..SpritesSummary::default()
    };
    let mut index = Vec::with_capacity(jobs.len());
    for (job, result) in jobs.iter().zip(results) {
        let (key, error) = match result {
            Ok((key, true)) => {
                summary.rendered += 1;
                (key, None)
            }
            Ok((key, false)) => {
                summary.reused += 1;
                (key, None)
            }
            Err((key, reason)) => {
                summary.failed += 1;
                (key, Some(reason))
            }
        };
        index.push(SpriteEntry {
            mesh: job.mesh.clone(),
            model: job.model.clone(),
            references: job.references,
            base_kinds: job.base_kinds.iter().cloned().collect(),
            key,
            error,
        });
    }
    let index_path = dir.join("index.json");
    let text = serde_json::to_string_pretty(&index).expect("plain data") + "\n";
    std::fs::write(&index_path, text).map_err(io(&index_path))?;
    Ok(summary)
}

/// Reads `index.json` back.
pub fn read_index(inputs: &Inputs) -> Result<Vec<SpriteEntry>, ConvertError> {
    let path = inputs.sprites_dir().join("index.json");
    let text = std::fs::read_to_string(&path).map_err(io(&path))?;
    serde_json::from_str(&text)
        .map_err(|e| ConvertError::stage("sprites", format!("index.json: {e}")))
}

/// Reads one sprite's metadata and FRM bytes.
pub fn read_sprite(inputs: &Inputs, key: &str) -> Result<(SpriteMeta, Vec<u8>), ConvertError> {
    let dir = inputs.sprites_dir();
    let meta_path = dir.join(format!("{key}.json"));
    let text = std::fs::read_to_string(&meta_path).map_err(io(&meta_path))?;
    let meta = serde_json::from_str(&text)
        .map_err(|e| ConvertError::stage("sprites", format!("{key}.json: {e}")))?;
    let frm_path = dir.join(format!("{key}.frm"));
    let bytes = std::fs::read(&frm_path).map_err(io(&frm_path))?;
    Ok((meta, bytes))
}
