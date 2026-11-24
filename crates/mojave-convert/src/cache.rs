//! Content-hash caches below the stage keys: the map stage's renders and
//! grounds, kept on disk across runs.
//!
//! A stage key (`cache/map.key`) says whether a whole stage can be skipped.
//! When it cannot, most of the stage's work is still the same as last time:
//! a declutter threshold, a builder option or one map of a layer changes
//! what is pruned, cut or assembled, not how a mesh looks. The units cached
//! here are:
//!
//! - **renders** ([`CachedRenderer`]): every call the slicer makes to the
//!   renderer (canonical renders of a reference, floor pieces, the wire
//!   test, the ground overlay bake, a window of each), keyed by the scene's
//!   content and the window;
//! - **grounds** ([`cached_ground`]): one terrain run's tiles, squares and
//!   statistics, keyed by everything `map_terrain::build_ground` reads;
//! - **map previews** ([`cached_png`]): the PNG of a preview whose pixels
//!   were encoded before;
//! - **decoded meshes and textures** ([`SharedAssets`], in memory): the
//!   extract already converted each mesh and texture once per content hash
//!   (`mir/meshes/<hash>.glb`, `mir/textures/<hash>.png`); a run reads and
//!   decodes each of those once, for the estimate and every slicer run of a
//!   layer, instead of once per run.
//!
//! Pruning, cutaway trimming, strip cutting, the budget ladder, the
//! catalogue and the builder run again from cached renders; they are
//! cheap next to rendering.
//!
//! # Keys
//!
//! A key is a SHA-256 over a revision constant of this module, the output
//! revision of the crate that does the work
//! (`foundry_render::OUTPUT_REVISION`, `map_terrain::OUTPUT_REVISION`), and
//! the content of every input: mesh vertex data and materials, transforms,
//! texture content hashes, the calibration, the backend, the projection,
//! the window, the ground sampler's digest, the overlays' pixels, the
//! palette and the options. Never a path, a time or a host name.
//!
//! # The store
//!
//! [`Store`] keeps one file per entry under `<dir>/<kind>/<2 hex>/<key>.bin`
//! (`<dir>` is `--cache-dir`, by default `<out>/cache`). A file is a header
//! (magic, kind, key, payload length, payload SHA-256) and the payload. A
//! read checks all five; an entry that fails any check (truncated, edited,
//! another key's file, an older format) is deleted and counted as corrupt,
//! and the work is redone. Files are written to a temporary name and
//! renamed, so a reader never sees half an entry, even with two runs
//! sharing one `--cache-dir`. Writes are best effort: a failed write costs
//! a later miss, never a wrong result. Nothing is ever evicted; delete the
//! directory to reclaim the space.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use foundry_render::{Backend, RenderError, RenderOutput, Renderer, Scene, Window};
use sha2::{Digest, Sha256};

/// This module's render key revision: bump when the render key or the
/// render encoding changes.
pub const RENDER_REVISION: &str = "render-cache-1";
/// This module's ground key revision: bump when the ground key or the
/// ground encoding changes.
pub const GROUND_REVISION: &str = "ground-cache-1";
/// This module's preview key revision: bump when the preview key changes
/// or `foundry_gallery::encode_png` writes other bytes.
pub const PREVIEW_REVISION: &str = "preview-cache-1";

/// A cache key.
pub type Key = [u8; 32];

/// The file header's magic.
const MAGIC: &[u8; 8] = b"MFCACHE1";

/// Hex of a key.
pub fn hex(key: &Key) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds a key from typed parts; every variable-length part is length
/// prefixed, so two different sequences of parts never hash alike.
pub struct KeyHasher(Sha256);

impl KeyHasher {
    /// A hasher for keys of `kind` (`"render"`, `"ground"`).
    pub fn new(kind: &str) -> Self {
        let mut hasher = KeyHasher(Sha256::new());
        hasher.str(kind);
        hasher
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
        self
    }

    pub fn str(&mut self, text: &str) -> &mut Self {
        self.bytes(text.as_bytes())
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.0.update(value.to_le_bytes());
        self
    }

    pub fn i64(&mut self, value: i64) -> &mut Self {
        self.0.update(value.to_le_bytes());
        self
    }

    pub fn f32(&mut self, value: f32) -> &mut Self {
        self.0.update(value.to_bits().to_le_bytes());
        self
    }

    pub fn bool(&mut self, value: bool) -> &mut Self {
        self.0.update([u8::from(value)]);
        self
    }

    pub fn finish(self) -> Key {
        self.0.finalize().into()
    }
}

/// What a [`Store`] saw, for the timings.
#[derive(Debug, Default)]
pub struct Counters {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub corrupt: AtomicU64,
    pub written: AtomicU64,
    pub bytes_read: AtomicU64,
    pub bytes_written: AtomicU64,
}

/// A directory of cache entries (module notes).
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    pub counters: Counters,
}

fn get(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

impl Store {
    /// The store in `dir` (created on the first write).
    pub fn open(dir: &Path) -> Self {
        Store {
            dir: dir.to_owned(),
            counters: Counters::default(),
        }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the entry `key` of `kind` lives.
    pub fn path(&self, kind: &str, key: &Key) -> PathBuf {
        let name = hex(key);
        self.dir
            .join(kind)
            .join(&name[..2])
            .join(format!("{name}.bin"))
    }

    /// The payload of entry `key`, when it is there and intact. A damaged
    /// entry is deleted and counted.
    pub fn get(&self, kind: &str, key: &Key) -> Option<Vec<u8>> {
        let path = self.path(kind, key);
        let Ok(bytes) = std::fs::read(&path) else {
            add(&self.counters.misses, 1);
            return None;
        };
        match check(&bytes, kind, key) {
            Some(payload) => {
                add(&self.counters.hits, 1);
                add(&self.counters.bytes_read, bytes.len() as u64);
                Some(payload.to_vec())
            }
            None => {
                self.discard(kind, key);
                None
            }
        }
    }

    /// Deletes entry `key` (a payload that did not decode), counting it
    /// as corrupt.
    pub fn discard(&self, kind: &str, key: &Key) {
        add(&self.counters.corrupt, 1);
        add(&self.counters.misses, 1);
        let _ = std::fs::remove_file(self.path(kind, key));
    }

    /// Writes entry `key`; best effort (module notes).
    pub fn put(&self, kind: &str, key: &Key, payload: &[u8]) {
        let path = self.path(kind, key);
        let Some(parent) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let mut file = Vec::with_capacity(payload.len() + 96);
        file.extend_from_slice(MAGIC);
        file.push(kind.len() as u8);
        file.extend_from_slice(kind.as_bytes());
        file.extend_from_slice(key);
        file.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        file.extend_from_slice(&Sha256::digest(payload));
        file.extend_from_slice(payload);
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let temporary = parent.join(format!(
            "{}.tmp-{}-{}",
            hex(key),
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        if std::fs::write(&temporary, &file).is_ok() && std::fs::rename(&temporary, &path).is_ok() {
            add(&self.counters.written, 1);
            add(&self.counters.bytes_written, file.len() as u64);
        } else {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    /// One line of counts.
    pub fn summary(&self) -> String {
        let c = &self.counters;
        format!(
            "{} hits ({:.1} MB read), {} misses, {} corrupt entries rebuilt, {} written ({:.1} MB)",
            get(&c.hits),
            get(&c.bytes_read) as f64 / 1.0e6,
            get(&c.misses),
            get(&c.corrupt),
            get(&c.written),
            get(&c.bytes_written) as f64 / 1.0e6
        )
    }
}

/// The payload of a file whose header is right for `kind` and `key`.
fn check<'a>(bytes: &'a [u8], kind: &str, key: &Key) -> Option<&'a [u8]> {
    let mut d = Dec::new(bytes);
    if d.take(MAGIC.len())? != MAGIC {
        return None;
    }
    let length = usize::from(d.u8()?);
    if d.take(length)? != kind.as_bytes() || d.take(32)? != key {
        return None;
    }
    let size = usize::try_from(d.u64()?).ok()?;
    let digest = d.take(32)?;
    let payload = d.take(size)?;
    if !d.done() || Sha256::digest(payload).as_slice() != digest {
        return None;
    }
    Some(payload)
}

// ---- Encoding ---------------------------------------------------------------

/// A payload being written.
#[derive(Default)]
pub struct Enc(pub Vec<u8>);

impl Enc {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    pub fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.0.extend_from_slice(v);
    }
    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
}

/// A payload being read; every read is checked, `None` past the end.
pub struct Dec<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Dec<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Dec { bytes, at: 0 }
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let out = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(out)
    }
    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Some(u64::from_le_bytes(a))
    }
    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    pub fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    pub fn f64(&mut self) -> Option<f64> {
        self.u64().map(f64::from_bits)
    }
    pub fn usize(&mut self) -> Option<usize> {
        usize::try_from(self.u64()?).ok()
    }
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.usize()?;
        self.take(n)
    }
    pub fn str(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }
    /// Whether every byte was read.
    pub fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

// ---- Renders -----------------------------------------------------------------

/// A render's payload: the header fields, then the pixels as runs of equal
/// `(rgba, depth)` pairs and literal blocks (the transparent background of
/// a render is one long run).
pub fn encode_render(out: &RenderOutput) -> Option<Vec<u8>> {
    let n = out.width as usize * out.height as usize;
    if out.rgba.len() != n * 4 || out.depth.len() != n {
        return None;
    }
    let mut e = Enc::default();
    e.i32(out.origin[0]);
    e.i32(out.origin[1]);
    e.u32(out.width);
    e.u32(out.height);
    e.u8(match out.backend {
        Backend::Auto => 0,
        Backend::Cpu => 1,
        Backend::Gpu => 2,
    });
    e.u32(out.triangles);
    e.u32(out.skipped_triangles);
    e.u32(out.missing_textures.len() as u32);
    for t in &out.missing_textures {
        e.str(t);
    }
    let pixel = |i: usize| -> u64 {
        let p = &out.rgba[i * 4..i * 4 + 4];
        u64::from(u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            | (u64::from(out.depth[i].to_bits()) << 32)
    };
    // A tag below 2^31 is a run of that many copies of the next pixel; a
    // tag with the top bit set is that many literal pixels.
    const LITERAL: u32 = 1 << 31;
    let mut i = 0;
    while i < n {
        let p = pixel(i);
        let mut run = 1;
        while i + run < n && run < (LITERAL - 1) as usize && pixel(i + run) == p {
            run += 1;
        }
        if run >= 3 {
            e.u32(run as u32);
            e.u64(p);
            i += run;
            continue;
        }
        // Literals up to the next run of three.
        let start = i;
        let mut end = i;
        while end < n && end - start < (LITERAL - 1) as usize {
            let q = pixel(end);
            if end + 2 < n && pixel(end + 1) == q && pixel(end + 2) == q {
                break;
            }
            end += 1;
        }
        let end = end.max(start + 1).min(n);
        e.u32(LITERAL | (end - start) as u32);
        for k in start..end {
            e.u64(pixel(k));
        }
        i = end;
    }
    Some(e.0)
}

/// [`encode_render`] read back; `None` for a payload that does not decode
/// to a whole render.
pub fn decode_render(payload: &[u8]) -> Option<RenderOutput> {
    let mut d = Dec::new(payload);
    let origin = [d.i32()?, d.i32()?];
    let width = d.u32()?;
    let height = d.u32()?;
    let backend = match d.u8()? {
        0 => Backend::Auto,
        1 => Backend::Cpu,
        2 => Backend::Gpu,
        _ => return None,
    };
    let triangles = d.u32()?;
    let skipped_triangles = d.u32()?;
    let missing = d.u32()? as usize;
    let mut missing_textures = Vec::with_capacity(missing.min(1024));
    for _ in 0..missing {
        missing_textures.push(d.str()?);
    }
    let n = usize::try_from(u64::from(width) * u64::from(height)).ok()?;
    // A count the payload cannot hold is refused before allocating.
    if n > payload.len().saturating_mul(1 << 31) {
        return None;
    }
    let mut rgba = Vec::with_capacity(n.saturating_mul(4).min(1 << 30));
    let mut depth = Vec::with_capacity(n.min(1 << 28));
    let push = |p: u64, rgba: &mut Vec<u8>, depth: &mut Vec<f32>| {
        rgba.extend_from_slice(&(p as u32).to_le_bytes());
        depth.push(f32::from_bits((p >> 32) as u32));
    };
    while depth.len() < n {
        let tag = d.u32()?;
        let count = (tag & !(1 << 31)) as usize;
        if count == 0 || depth.len() + count > n {
            return None;
        }
        if tag & (1 << 31) == 0 {
            let p = d.u64()?;
            for _ in 0..count {
                push(p, &mut rgba, &mut depth);
            }
        } else {
            for _ in 0..count {
                let p = d.u64()?;
                push(p, &mut rgba, &mut depth);
            }
        }
    }
    if !d.done() {
        return None;
    }
    Some(RenderOutput {
        origin,
        width,
        height,
        rgba,
        depth,
        backend,
        missing_textures,
        triangles,
        skipped_triangles,
    })
}

/// Hashes everything of a mesh a renderer reads: per primitive its
/// positions, normals, texture coordinates, vertex colours, indices,
/// material and whether it is well formed (names, collision and skins are
/// never drawn).
pub fn hash_mesh(h: &mut KeyHasher, mesh: &nif::StaticMesh) {
    h.u64(mesh.primitives.len() as u64);
    for p in &mesh.primitives {
        h.bool(p.is_well_formed());
        h.u64(p.positions.len() as u64);
        for v in &p.positions {
            v.iter().for_each(|c| {
                h.f32(*c);
            });
        }
        match &p.normals {
            Some(n) => {
                h.bool(true).u64(n.len() as u64);
                for v in n {
                    v.iter().for_each(|c| {
                        h.f32(*c);
                    });
                }
            }
            None => {
                h.bool(false);
            }
        }
        match &p.uvs {
            Some(n) => {
                h.bool(true).u64(n.len() as u64);
                for v in n {
                    v.iter().for_each(|c| {
                        h.f32(*c);
                    });
                }
            }
            None => {
                h.bool(false);
            }
        }
        match &p.colors {
            Some(n) => {
                h.bool(true).u64(n.len() as u64);
                for v in n {
                    v.iter().for_each(|c| {
                        h.f32(*c);
                    });
                }
            }
            None => {
                h.bool(false);
            }
        }
        h.u64(p.indices.len() as u64);
        let mut indices = Vec::with_capacity(p.indices.len() * 4);
        for i in &p.indices {
            indices.extend_from_slice(&i.to_le_bytes());
        }
        h.bytes(&indices);
        h.str(&format!("{:?}", p.material));
        h.bool(p.skinned);
    }
}

/// A renderer that keeps every output in a [`Store`] (module notes).
///
/// Textures are identified by `texture_id` (path to content hash, `None`
/// for a missing one): it must name the content of exactly what the
/// scene's [`foundry_render::TextureSource`] returns for that path, as
/// `mir::Mir::texture_hash` does for the slicer's MIR textures.
pub struct CachedRenderer<'a> {
    inner: &'a mut dyn Renderer,
    store: &'a Store,
    salt: Key,
    texture_id: &'a dyn Fn(&str) -> Option<String>,
    /// Renders done by the inner renderer, and their seconds.
    pub rendered: usize,
    pub render_time: Duration,
    /// Renders read from the store.
    pub reused: usize,
    /// Seconds spent hashing scenes.
    pub key_time: Duration,
}

impl<'a> CachedRenderer<'a> {
    /// Wraps `inner`, a renderer built from the calibration whose TOML is
    /// `calibration`.
    pub fn new(
        inner: &'a mut dyn Renderer,
        calibration: &str,
        store: &'a Store,
        texture_id: &'a dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let mut salt = KeyHasher::new("render-salt");
        salt.str(RENDER_REVISION)
            .u64(u64::from(foundry_render::OUTPUT_REVISION))
            .str(calibration)
            .str(crate::sprites::backend_name(inner.backend()));
        CachedRenderer {
            inner,
            store,
            salt: salt.finish(),
            texture_id,
            rendered: 0,
            render_time: Duration::ZERO,
            reused: 0,
            key_time: Duration::ZERO,
        }
    }

    /// The key of rendering `scene` into `window`.
    pub fn key(&self, scene: &Scene, window: Window) -> Key {
        let mut h = KeyHasher::new("render");
        h.bytes(&self.salt);
        match window {
            Window::Fit => {
                h.u64(0);
            }
            Window::Rect {
                x,
                y,
                width,
                height,
            } => {
                h.u64(1)
                    .i64(i64::from(x))
                    .i64(i64::from(y))
                    .u64(u64::from(width))
                    .u64(u64::from(height));
            }
        }
        scene.anchor.iter().for_each(|v| {
            h.f32(*v);
        });
        h.u64(scene.instances.len() as u64);
        for instance in &scene.instances {
            hash_mesh(&mut h, instance.mesh);
            for row in &instance.transform {
                row.iter().for_each(|v| {
                    h.f32(*v);
                });
            }
            h.str(&format!("{:?}", instance.shading));
            h.bool(instance.two_sided);
            match instance.alpha_cutoff {
                Some(c) => h.bool(true).f32(c),
                None => h.bool(false),
            };
            for path in instance.mesh.texture_paths() {
                h.str(&path);
                h.str((self.texture_id)(&path).as_deref().unwrap_or("missing"));
            }
        }
        h.finish()
    }

    /// Adds a note of what it did.
    pub fn note(&self, what: &str) {
        crate::timings::note(format!(
            "{what}: {} renders reused, {} rendered in {:.2} s, keys {:.2} s",
            self.reused,
            self.rendered,
            self.render_time.as_secs_f64(),
            self.key_time.as_secs_f64()
        ));
    }
}

impl Renderer for CachedRenderer<'_> {
    fn render(&mut self, scene: &Scene, window: Window) -> Result<RenderOutput, RenderError> {
        let started = Instant::now();
        let key = self.key(scene, window);
        self.key_time += started.elapsed();
        if let Some(payload) = self.store.get("render", &key) {
            match decode_render(&payload) {
                Some(out) => {
                    self.reused += 1;
                    return Ok(out);
                }
                None => self.store.discard("render", &key),
            }
        }
        let started = Instant::now();
        let out = self.inner.render(scene, window)?;
        self.render_time += started.elapsed();
        self.rendered += 1;
        if let Some(payload) = encode_render(&out) {
            self.store.put("render", &key, &payload);
        }
        Ok(out)
    }

    fn backend(&self) -> Backend {
        self.inner.backend()
    }
}

// ---- Decoded assets ----------------------------------------------------------

type MeshSlot = Result<Rc<nif::StaticMesh>, String>;
/// Decoded textures by content hash, and their bytes.
type TextureCache = (BTreeMap<String, Option<Rc<foundry_render::Texture>>>, usize);

/// [`map_slicer::Assets`] over a MIR directory that reads each mesh and
/// decodes each texture once per content hash for as long as it lives
/// (one map stage), whatever the number of slicer runs and estimates that
/// ask. Textures are kept while their decoded bytes stay under a limit
/// (the slicer's `texture_cache_bytes`); past it the texture cache starts
/// again, as the slicer's own does.
pub struct SharedAssets<'a> {
    mir: &'a mir::Mir,
    meshes: RefCell<BTreeMap<String, MeshSlot>>,
    textures: SharedTextures<'a>,
    /// Seconds spent reading meshes; meshes read and reused.
    pub mesh_time: Cell<Duration>,
    pub meshes_read: Cell<usize>,
    pub meshes_reused: Cell<usize>,
}

struct SharedTextures<'a> {
    mir: &'a mir::Mir,
    limit: usize,
    /// By content hash (or path, for a path the MIR does not know).
    cache: RefCell<TextureCache>,
    decode_time: Cell<Duration>,
    decoded: Cell<usize>,
}

impl<'a> SharedAssets<'a> {
    /// The assets of `mir`, keeping at most `texture_bytes` of decoded
    /// textures.
    pub fn new(mir: &'a mir::Mir, texture_bytes: usize) -> Self {
        SharedAssets {
            mir,
            meshes: RefCell::new(BTreeMap::new()),
            textures: SharedTextures {
                mir,
                limit: texture_bytes,
                cache: RefCell::new((BTreeMap::new(), 0)),
                decode_time: Cell::new(Duration::ZERO),
                decoded: Cell::new(0),
            },
            mesh_time: Cell::new(Duration::ZERO),
            meshes_read: Cell::new(0),
            meshes_reused: Cell::new(0),
        }
    }

    /// Adds a note of what it did.
    pub fn note(&self) {
        crate::timings::note(format!(
            "assets: {} meshes read in {:.2} s, {} reused; {} textures decoded in {:.2} s",
            self.meshes_read.get(),
            self.mesh_time.get().as_secs_f64(),
            self.meshes_reused.get(),
            self.textures.decoded.get(),
            self.textures.decode_time.get().as_secs_f64()
        ));
    }
}

impl map_slicer::Assets for SharedAssets<'_> {
    fn mesh(&self, hash: &str) -> Result<nif::StaticMesh, String> {
        if let Some(slot) = self.meshes.borrow().get(hash) {
            self.meshes_reused.set(self.meshes_reused.get() + 1);
            return slot.as_ref().map(|m| (**m).clone()).map_err(Clone::clone);
        }
        let started = Instant::now();
        let slot: MeshSlot = self.mir.mesh(hash).map(Rc::new).map_err(|e| e.to_string());
        self.mesh_time.set(self.mesh_time.get() + started.elapsed());
        self.meshes_read.set(self.meshes_read.get() + 1);
        let out = slot.as_ref().map(|m| (**m).clone()).map_err(Clone::clone);
        self.meshes.borrow_mut().insert(hash.to_owned(), slot);
        out
    }

    fn textures(&self) -> &dyn foundry_render::TextureSource {
        &self.textures
    }
}

impl foundry_render::TextureSource for SharedTextures<'_> {
    fn texture(&self, path: &str) -> Option<Cow<'_, foundry_render::Texture>> {
        let hash = self.mir.texture_hash(path);
        let name = hash.map_or_else(|| format!("path {path}"), str::to_owned);
        let mut guard = self.cache.borrow_mut();
        let (cache, bytes) = &mut *guard;
        if let Some(slot) = cache.get(&name) {
            return slot.as_ref().map(|t| Cow::Owned((**t).clone()));
        }
        let started = Instant::now();
        let texture = hash
            .and_then(|hash| self.mir.texture(hash).ok())
            .map(|image| {
                Rc::new(foundry_render::Texture {
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba,
                })
            });
        self.decode_time
            .set(self.decode_time.get() + started.elapsed());
        self.decoded.set(self.decoded.get() + 1);
        let size = texture.as_ref().map_or(0, |t| t.rgba.len());
        if *bytes + size > self.limit {
            cache.clear();
            *bytes = 0;
        }
        *bytes += size;
        cache.insert(name, texture.clone());
        texture.map(|t| Cow::Owned((*t).clone()))
    }
}

// ---- Previews ----------------------------------------------------------------

/// The PNG of an indexed map preview: from the store when the same pixels
/// and palette were encoded before, else encoded by `encode` and stored.
pub fn cached_png<E>(
    store: &Store,
    indexed: &[u8],
    width: u32,
    height: u32,
    palette: &[u8],
    encode: impl FnOnce() -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, E> {
    let mut h = KeyHasher::new("preview");
    h.str(PREVIEW_REVISION)
        .u64(u64::from(width))
        .u64(u64::from(height))
        .bytes(&Sha256::digest(palette))
        .bytes(&Sha256::digest(indexed));
    let key = h.finish();
    if let Some(png) = store.get("preview", &key) {
        return Ok(png);
    }
    let png = encode()?;
    store.put("preview", &key, &png);
    Ok(png)
}

// ---- Grounds -----------------------------------------------------------------

/// A ground's payload.
pub fn encode_ground(ground: &map_terrain::Ground) -> Vec<u8> {
    let mut e = Enc::default();
    e.u64(ground.tiles.len() as u64);
    for tile in &ground.tiles {
        e.bytes(tile);
    }
    e.u64(ground.squares.len() as u64);
    for q in &ground.squares {
        e.0.extend_from_slice(&q.to_le_bytes());
    }
    e.u64(ground.blank_squares.len() as u64);
    for b in &ground.blank_squares {
        e.u8(u8::from(*b));
    }
    let s = &ground.stats;
    let us = |e: &mut Enc, v: usize| e.u64(v as u64);
    us(&mut e, s.tiles_before);
    us(&mut e, s.distinct_tiles);
    us(&mut e, s.exact_duplicates);
    us(&mut e, s.clusters);
    e.f32(s.texture_repeat_used);
    us(&mut e, s.phase_period[0]);
    us(&mut e, s.phase_period[1]);
    us(&mut e, s.tiles_after);
    us(&mut e, s.substituted_squares);
    e.f64(s.mean_substitution_error);
    e.f64(s.worst_substitution_error);
    e.u64(s.fallback_pixels);
    us(&mut e, s.blank_squares);
    e.u64(s.missing_texture_pixels);
    us(&mut e, s.missing_texture_layers);
    us(&mut e, s.unnamed_texture_layers);
    us(&mut e, s.borrowed_base_layers);
    e.u64(s.defaulted_texture_pixels);
    us(&mut e, s.defaulted_texture_layers);
    us(&mut e, s.textures_loaded);
    us(&mut e, s.textures_missing);
    us(&mut e, s.cells_with_land);
    us(&mut e, s.cells_without_land);
    for d in [s.load_time, s.evaluate_time, s.deduplicate_time] {
        e.u64(d.as_nanos().min(u128::from(u64::MAX)) as u64);
    }
    e.0
}

/// [`encode_ground`] read back.
pub fn decode_ground(payload: &[u8]) -> Option<map_terrain::Ground> {
    let mut d = Dec::new(payload);
    let count = d.usize()?;
    if count > payload.len() {
        return None;
    }
    let mut tiles = Vec::with_capacity(count);
    for _ in 0..count {
        tiles.push(d.bytes()?.to_vec());
    }
    let count = d.usize()?;
    let raw = d.take(count.checked_mul(2)?)?;
    let squares = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let count = d.usize()?;
    let blank_squares = d
        .take(count)?
        .iter()
        .map(|b| match b {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        })
        .collect::<Option<Vec<bool>>>()?;
    let stats = map_terrain::GroundStats {
        tiles_before: d.usize()?,
        distinct_tiles: d.usize()?,
        exact_duplicates: d.usize()?,
        clusters: d.usize()?,
        texture_repeat_used: d.f32()?,
        phase_period: [d.usize()?, d.usize()?],
        tiles_after: d.usize()?,
        substituted_squares: d.usize()?,
        mean_substitution_error: d.f64()?,
        worst_substitution_error: d.f64()?,
        fallback_pixels: d.u64()?,
        blank_squares: d.usize()?,
        missing_texture_pixels: d.u64()?,
        missing_texture_layers: d.usize()?,
        unnamed_texture_layers: d.usize()?,
        borrowed_base_layers: d.usize()?,
        defaulted_texture_pixels: d.u64()?,
        defaulted_texture_layers: d.usize()?,
        textures_loaded: d.usize()?,
        textures_missing: d.usize()?,
        cells_with_land: d.usize()?,
        cells_without_land: d.usize()?,
        load_time: Duration::from_nanos(d.u64()?),
        evaluate_time: Duration::from_nanos(d.u64()?),
        deduplicate_time: Duration::from_nanos(d.u64()?),
    };
    d.done().then_some(map_terrain::Ground {
        tiles,
        squares,
        blank_squares,
        stats,
    })
}

/// What a ground depends on besides its land (module notes).
pub struct GroundKey<'a> {
    /// `Some(manifest digest, worldspace)` for the worldspace's land; `None`
    /// for no land (a blank, flat floor).
    pub land: Option<(&'a str, &'a str)>,
    pub window: &'a mir::MapWindow,
    pub sampler: &'a mir::GroundSampler,
    pub projection: &'a foundry_render::Projection,
    /// The calibration's TOML.
    pub calibration: &'a str,
    pub palette: &'a [u8],
    pub overlays: &'a [map_terrain::Overlay<'a>],
    pub options: &'a map_terrain::TerrainOptions,
}

impl GroundKey<'_> {
    /// The key.
    pub fn key(&self) -> Key {
        let mut h = KeyHasher::new("ground");
        h.str(GROUND_REVISION)
            .u64(u64::from(map_terrain::OUTPUT_REVISION));
        match self.land {
            Some((manifest, worldspace)) => {
                h.bool(true)
                    .str(manifest)
                    .str(&worldspace.to_ascii_lowercase());
            }
            None => {
                h.bool(false);
            }
        }
        h.str(&format!("{:?}", self.window))
            .str(&self.sampler.digest())
            .str(&format!("{:?}", self.projection))
            .str(self.calibration)
            .bytes(&Sha256::digest(self.palette));
        // `jobs` never changes the output.
        let options = map_terrain::TerrainOptions {
            jobs: 1,
            ..self.options.clone()
        };
        h.str(&format!("{options:?}"));
        h.u64(self.overlays.len() as u64);
        for o in self.overlays {
            h.i64(i64::from(o.origin[0]))
                .i64(i64::from(o.origin[1]))
                .u64(u64::from(o.width))
                .u64(u64::from(o.height))
                .bytes(&Sha256::digest(o.rgba));
        }
        h.finish()
    }
}

/// The ground `key` names, from the store or built now and stored. The
/// second value is whether it came from the store.
pub fn cached_ground<E>(
    store: &Store,
    key: &GroundKey<'_>,
    build: impl FnOnce() -> Result<map_terrain::Ground, E>,
) -> Result<(map_terrain::Ground, bool), E> {
    let key = key.key();
    if let Some(payload) = store.get("ground", &key) {
        match decode_ground(&payload) {
            Some(ground) => return Ok((ground, true)),
            None => store.discard("ground", &key),
        }
    }
    let ground = build()?;
    store.put("ground", &key, &encode_ground(&ground));
    Ok((ground, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(width: u32, height: u32) -> RenderOutput {
        let n = (width * height) as usize;
        let mut rgba = vec![0u8; n * 4];
        let mut depth = vec![f32::INFINITY; n];
        for i in (0..n).filter(|i| i % 7 < 3) {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[i as u8, 2, 3, 255]);
            depth[i] = i as f32 * 0.25 - 3.0;
        }
        RenderOutput {
            origin: [-12, 40],
            width,
            height,
            rgba,
            depth,
            backend: Backend::Cpu,
            missing_textures: vec!["textures\\a.dds".to_owned()],
            triangles: 17,
            skipped_triangles: 2,
        }
    }

    #[test]
    fn a_render_round_trips_bit_for_bit() {
        for (w, h) in [(0, 0), (1, 1), (3, 1), (64, 37), (200, 3)] {
            let out = output(w, h);
            let payload = encode_render(&out).expect("encodes");
            let back = decode_render(&payload).expect("decodes");
            assert_eq!(back.rgba, out.rgba);
            let bits = |d: &[f32]| d.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            assert_eq!(bits(&back.depth), bits(&out.depth));
            assert_eq!(
                (back.origin, back.width, back.height, back.triangles),
                (out.origin, out.width, out.height, out.triangles)
            );
            assert_eq!(back.missing_textures, out.missing_textures);
            // A cut payload never decodes.
            if !payload.is_empty() {
                assert!(decode_render(&payload[..payload.len() - 1]).is_none());
            }
        }
        // The background compresses.
        let mut empty = output(256, 256);
        empty.rgba.iter_mut().for_each(|b| *b = 0);
        empty.depth.iter_mut().for_each(|d| *d = f32::INFINITY);
        assert!(encode_render(&empty).expect("encodes").len() < 200);
    }

    #[test]
    fn a_damaged_entry_is_dropped_and_counted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open(dir.path());
        let key = KeyHasher::new("t").finish();
        assert!(store.get("render", &key).is_none());
        store.put("render", &key, b"payload bytes");
        assert_eq!(
            store.get("render", &key).as_deref(),
            Some(&b"payload bytes"[..])
        );
        let path = store.path("render", &key);
        let whole = std::fs::read(&path).expect("written");
        // Truncated, one byte flipped, another key's file, another kind.
        let mut flipped = whole.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        let other = KeyHasher::new("u").finish();
        for damaged in [whole[..whole.len() - 3].to_vec(), flipped] {
            std::fs::write(&path, &damaged).expect("damage");
            assert!(store.get("render", &key).is_none());
            assert!(!path.exists(), "a damaged entry is deleted");
            store.put("render", &key, b"payload bytes");
        }
        std::fs::create_dir_all(store.path("render", &other).parent().expect("parent")).ok();
        std::fs::write(store.path("render", &other), &whole).expect("copy");
        assert!(store.get("render", &other).is_none());
        std::fs::create_dir_all(store.path("ground", &key).parent().expect("parent")).ok();
        std::fs::write(store.path("ground", &key), &whole).expect("copy");
        assert!(store.get("ground", &key).is_none());
        assert_eq!(store.counters.corrupt.load(Ordering::Relaxed), 4);
        // The intact entry still reads.
        assert!(store.get("render", &key).is_some());
    }

    /// A renderer that must never be called.
    struct Never;
    impl Renderer for Never {
        fn render(&mut self, _: &Scene, _: Window) -> Result<RenderOutput, RenderError> {
            unreachable!("only keys are made")
        }
        fn backend(&self) -> Backend {
            Backend::Cpu
        }
    }

    fn mesh() -> nif::StaticMesh {
        let material = nif::Material {
            textures: vec!["textures\\wall.dds".to_owned()],
            ..nif::Material::default()
        };
        nif::StaticMesh {
            primitives: vec![nif::Primitive {
                positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 5.0]],
                uvs: Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
                indices: vec![0, 1, 2],
                material,
                ..nif::Primitive::default()
            }],
            ..nif::StaticMesh::default()
        }
    }

    #[test]
    fn a_render_key_follows_every_input_and_nothing_else() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::open(dir.path());
        let textures = foundry_render::NoTextures;
        let base_mesh = mesh();
        let id_a = |_: &str| Some("aa".to_owned());
        let id_b = |_: &str| Some("bb".to_owned());
        let window = Window::Rect {
            x: -4,
            y: 2,
            width: 64,
            height: 32,
        };
        let key = |mesh: &nif::StaticMesh,
                   calibration: &str,
                   texture_id: &dyn Fn(&str) -> Option<String>,
                   edit: &dyn Fn(&mut foundry_render::Instance<'_>),
                   anchor: [f32; 3],
                   window: Window| {
            let mut never = Never;
            let cached = CachedRenderer::new(&mut never, calibration, &store, texture_id);
            let mut instance =
                foundry_render::Instance::new(mesh, foundry_render::Instance::IDENTITY, &textures);
            edit(&mut instance);
            cached.key(
                &Scene {
                    anchor,
                    instances: vec![instance],
                },
                window,
            )
        };
        let same = |_: &mut foundry_render::Instance<'_>| {};
        let base = key(&base_mesh, "c", &id_a, &same, [0.0; 3], window);
        assert_eq!(base, key(&base_mesh, "c", &id_a, &same, [0.0; 3], window));
        // A mesh built again from the same data is the same mesh.
        assert_eq!(base, key(&mesh(), "c", &id_a, &same, [0.0; 3], window));
        let mut moved = mesh();
        moved.primitives[0].positions[2][2] = 5.5;
        let mut material = mesh();
        material.primitives[0].material.two_sided = true;
        let mut renamed = mesh();
        renamed.primitives[0].name = "never drawn".to_owned();
        assert_eq!(base, key(&renamed, "c", &id_a, &same, [0.0; 3], window));
        let changed = [
            key(&moved, "c", &id_a, &same, [0.0; 3], window),
            key(&material, "c", &id_a, &same, [0.0; 3], window),
            key(
                &base_mesh,
                "other calibration",
                &id_a,
                &same,
                [0.0; 3],
                window,
            ),
            key(&base_mesh, "c", &id_b, &same, [0.0; 3], window),
            key(
                &base_mesh,
                "c",
                &id_a,
                &|i| i.transform[0][3] = 1.0,
                [0.0; 3],
                window,
            ),
            key(
                &base_mesh,
                "c",
                &id_a,
                &|i| i.two_sided = true,
                [0.0; 3],
                window,
            ),
            key(
                &base_mesh,
                "c",
                &id_a,
                &|i| i.alpha_cutoff = Some(0.1),
                [0.0; 3],
                window,
            ),
            key(
                &base_mesh,
                "c",
                &id_a,
                &|i| i.shading = foundry_render::Shading::Unlit,
                [0.0; 3],
                window,
            ),
            key(&base_mesh, "c", &id_a, &same, [0.0, 0.0, 1.0], window),
            key(&base_mesh, "c", &id_a, &same, [0.0; 3], Window::Fit),
        ];
        let distinct: std::collections::BTreeSet<Key> = changed.iter().copied().collect();
        assert_eq!(distinct.len(), changed.len());
        assert!(!distinct.contains(&base));
    }

    #[test]
    fn a_ground_key_follows_its_inputs_but_not_the_jobs() {
        let window = mir::MapWindow {
            x_west: 0.0,
            y_north: 4096.0,
            size: 4096.0,
        };
        let projection = foundry_render::Projection::new(&foundry_render::Calibration::default())
            .expect("a projection");
        let flat = mir::GroundSampler::flat(10.0);
        let other_floor = mir::GroundSampler::flat(12.0);
        let rgba = vec![200u8; 16];
        let mut other_rgba = rgba.clone();
        other_rgba[5] = 1;
        fn overlay(rgba: &[u8]) -> Vec<map_terrain::Overlay<'_>> {
            vec![map_terrain::Overlay {
                origin: [3, 4],
                width: 2,
                height: 2,
                rgba,
            }]
        }
        let base_overlays = overlay(&rgba);
        let other_overlays = overlay(&other_rgba);
        let options = map_terrain::TerrainOptions::default();
        let more_jobs = map_terrain::TerrainOptions {
            jobs: 8,
            ..options.clone()
        };
        let cap = map_terrain::TerrainOptions {
            library_cap: 99,
            ..options.clone()
        };
        let key = |land: Option<(&str, &str)>,
                   sampler: &mir::GroundSampler,
                   overlays: &[map_terrain::Overlay<'_>],
                   options: &map_terrain::TerrainOptions,
                   palette: &[u8]| {
            GroundKey {
                land,
                window: &window,
                sampler,
                projection: &projection,
                calibration: "c",
                palette,
                overlays,
                options,
            }
            .key()
        };
        let base = key(None, &flat, &base_overlays, &options, b"p");
        assert_eq!(base, key(None, &flat, &base_overlays, &more_jobs, b"p"));
        let changed = [
            key(Some(("m", "w")), &flat, &base_overlays, &options, b"p"),
            key(None, &other_floor, &base_overlays, &options, b"p"),
            key(None, &flat, &other_overlays, &options, b"p"),
            key(None, &flat, &[], &options, b"p"),
            key(None, &flat, &base_overlays, &cap, b"p"),
            key(None, &flat, &base_overlays, &options, b"q"),
        ];
        let distinct: std::collections::BTreeSet<Key> = changed.iter().copied().collect();
        assert_eq!(distinct.len(), changed.len());
        assert!(!distinct.contains(&base));
    }

    #[test]
    fn a_ground_round_trips() {
        let ground = map_terrain::Ground {
            tiles: vec![vec![0; 4], vec![1, 2, 3, 4]],
            squares: vec![0, 1, 1, 0],
            blank_squares: vec![true, false, false, true],
            stats: map_terrain::GroundStats {
                distinct_tiles: 2,
                mean_substitution_error: 0.25,
                load_time: Duration::from_millis(3),
                ..map_terrain::GroundStats::default()
            },
        };
        let payload = encode_ground(&ground);
        assert_eq!(decode_ground(&payload), Some(ground));
        assert!(decode_ground(&payload[..payload.len() - 1]).is_none());
    }

    #[test]
    fn keys_hash_parts_unambiguously() {
        let mut a = KeyHasher::new("k");
        a.str("ab").str("c");
        let mut b = KeyHasher::new("k");
        b.str("a").str("bc");
        assert_ne!(a.finish(), b.finish());
    }
}
