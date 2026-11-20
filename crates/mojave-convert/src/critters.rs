//! The critter stage: every distinct human appearance among the slice's
//! actors is posed frame by frame, rendered in the six hex directions,
//! quantized to the Fallout 2 palette and written as one sprite set.
//!
//! Output, under `<out>/critters/`:
//!
//! - `<key>/aa.frm` (stand, from the idle animation), `<key>/ab.frm` (walk)
//!   and the placeholder animations of contracts section 10.4, each one
//!   still frame of the stand; `<key>/set.json` is written last and marks
//!   the set complete, which is the incremental cache. `<key>` is the
//!   content hash of everything that determines the set.
//! - `index.json`: one entry per actor document, sorted by form.
//!
//! A frame is the actor's parts skinned to one pose of the skeleton and
//! turned to face a hex direction; the walk is rendered in place and its
//! travel handed to `emit`, which turns it into frame offsets along that
//! direction's hex step.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use foundry_render::{Backend, Instance, Projection, Scene, Window};
use mir::{ActorDoc, ActorKind, Mir};
use quantize::{Dither, PaletteMap};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vault_formats::frm::FrmArchive;

use crate::config::{ConvertError, Inputs};
use crate::sprites::MirTextures;

/// Bumped whenever the stage's output for the same inputs would change.
const STAGE_VERSION: &str = "critters-3";
/// Frames a second of the stand; the idle is slow, and a set's size is
/// mostly its stand.
const STAND_FPS: u16 = 5;
/// Frames a second of the walk: the engine's default rate.
const WALK_FPS: u16 = 10;
/// How much wider than life a figure is drawn, about its own upright axis.
/// Fallout 2's figures are stocky; at the map's scale a true human reads
/// thin beside them (owner playtest, 2026-11-20).
const GIRTH: f32 = 1.15;
/// Gamma applied to a rendered figure before it is quantized; below 1
/// lifts the mid-tones. The source's outfits are dark, and at this size
/// they read as silhouettes (same playtest).
const LIFT_GAMMA: f32 = 0.78;
/// The stand's file suffix.
pub const STAND: &str = "aa";
/// The walk's file suffix.
pub const WALK: &str = "ab";
/// Animations a fight asks for, each one still frame of the stand until
/// real ones exist: hit, the two falls, the two get-ups, the two corpses,
/// punch and kick.
pub const PLACEHOLDERS: [&str; 9] = ["ao", "ba", "bb", "ch", "cj", "ra", "rb", "aq", "ar"];

/// What `<key>/set.json` records about one sprite set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CritterSetMeta {
    pub appearance: String,
    /// The first actor that needed the set, for people reading the file.
    pub label: String,
    pub stand_frames: usize,
    pub walk_frames: usize,
    /// Source units the walk travels in one cycle.
    pub walk_cycle_units: f32,
    /// Every animation suffix written.
    pub animations: Vec<String>,
    /// Texture paths the parts name that the extraction does not hold.
    pub missing_textures: Vec<String>,
}

/// One row of `index.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CritterEntry {
    /// The actor base's form key.
    pub actor: String,
    pub editor_id: Option<String>,
    pub name: Option<String>,
    pub female: bool,
    pub health: i32,
    /// The sprite set's key; `<key>/set.json` exists when `error` is `None`.
    pub set: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrittersSummary {
    pub actors: usize,
    pub sets: usize,
    pub rendered: usize,
    pub reused: usize,
    pub failed: usize,
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

/// The source yaw (a reference's rotation about Z) that faces each hex
/// direction, in FRM order: NE, E, SE, SW, W, NW.
///
/// The mapping is the map builder's own, so a critter is rendered facing
/// exactly the way it is placed.
pub fn direction_yaws(inputs: &Inputs) -> Result<[f32; 6], ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("critters", e);
    let projection = Projection::new(&inputs.calibration).map_err(|e| stage(&e))?;
    let window = mir::MapWindow::from_center(inputs.slice.center(), projection.tile_units());
    let frame = map_builder::walk::Frame::new(&window, &projection).map_err(|e| stage(&e))?;
    let mut yaws = [0.0f32; 6];
    for (direction, yaw) in yaws.iter_mut().enumerate() {
        *yaw = frame.yaw_of_orientation(direction as u8).ok_or_else(|| {
            stage(&format!(
                "hex direction {direction} has no ground direction"
            ))
        })?;
    }
    Ok(yaws)
}

/// The transform of a figure facing `yaw` at `scale`: widened by [`GIRTH`]
/// about its upright axis, then turned.
fn figure_transform(yaw: f32, scale: f32) -> [[f32; 4]; 3] {
    let mut matrix = mir::Transform {
        position: [0.0; 3],
        rotation: [0.0, 0.0, yaw],
        scale,
    }
    .matrix();
    for row in &mut matrix {
        row[0] *= GIRTH;
        row[1] *= GIRTH;
    }
    matrix
}

/// Lifts the mid-tones of every opaque pixel by [`LIFT_GAMMA`].
fn lift(rgba: &mut [u8]) {
    let table: [u8; 256] = std::array::from_fn(|v| {
        ((v as f32 / 255.0).powf(LIFT_GAMMA) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    });
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] != 0 {
            for channel in &mut pixel[..3] {
                *channel = table[usize::from(*channel)];
            }
        }
    }
}

/// The animation times of `frames` evenly spaced frames over `duration`.
fn frame_times(duration: f32, fps: u16) -> Vec<f32> {
    let count = ((duration * f32::from(fps)).round() as usize).max(1);
    (0..count)
        .map(|k| duration * k as f32 / count as f32)
        .collect()
}

struct Shared<'a> {
    mir: &'a Mir,
    map: &'a PaletteMap,
    projection: Projection,
    yaws: [f32; 6],
    /// Everything but the actor, hashed once.
    key_prefix: Vec<u8>,
    dir: PathBuf,
}

/// The bone rigid head pieces (hair, hats) ride.
const HEAD_BONE: &str = "Bip01 Head";

/// One mesh of an actor's body.
struct Part {
    mesh: nif::StaticMesh,
    /// Drawn with the hair colour multiplied in.
    tinted: bool,
}

/// An actor's parts, ready to pose.
struct Body {
    skeleton: nif::Skeleton,
    stand: nif::Animation,
    walk: nif::Animation,
    parts: Vec<Part>,
    head: Option<u32>,
    /// The skeleton's own pose.
    rest: nif::Pose,
    hair: Option<[u8; 3]>,
}

impl Body {
    /// Every part in `pose`: skinned parts through their skin, rigid ones
    /// (hair, hats, which are modelled around the head bone) carried by
    /// the head.
    fn posed(&self, pose: &nif::Pose) -> Vec<nif::StaticMesh> {
        self.parts
            .iter()
            .map(|part| {
                if part.mesh.primitives.iter().any(|p| p.skin.is_some()) {
                    return nif::skin(&part.mesh, &self.skeleton, pose);
                }
                let Some(matrix) = self.head.and_then(|head| {
                    // A rigid piece is modelled upright around the head
                    // bone's origin, not in the bone's own turned axes:
                    // undo the bone's rest rotation, then follow the pose.
                    let posed = pose.world.get(head as usize)?;
                    let rest = self.rest.world.get(head as usize)?;
                    let mut matrix = *posed;
                    for r in 0..3 {
                        for c in 0..3 {
                            matrix[r][c] = (0..3).map(|k| posed[r][k] * rest[c][k]).sum::<f32>();
                        }
                    }
                    Some(matrix)
                }) else {
                    return part.mesh.clone();
                };
                let matrix = &matrix;
                let mut mesh = part.mesh.clone();
                for primitive in &mut mesh.primitives {
                    for p in &mut primitive.positions {
                        *p = [0, 1, 2].map(|r| {
                            matrix[r][0] * p[0]
                                + matrix[r][1] * p[1]
                                + matrix[r][2] * p[2]
                                + matrix[r][3]
                        });
                    }
                    for n in primitive.normals.iter_mut().flatten() {
                        let v = [0, 1, 2].map(|r| {
                            matrix[r][0] * n[0] + matrix[r][1] * n[1] + matrix[r][2] * n[2]
                        });
                        let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                        if length > 0.0 {
                            *n = v.map(|c| c / length);
                        }
                    }
                }
                mesh
            })
            .collect()
    }

    /// The scene of `posed` parts turned by `transform`.
    fn scene<'a>(
        &self,
        posed: &'a [nif::StaticMesh],
        transform: [[f32; 4]; 3],
        plain: &'a dyn foundry_render::TextureSource,
        tinted: &'a dyn foundry_render::TextureSource,
    ) -> Scene<'a> {
        Scene {
            anchor: [0.0; 3],
            instances: posed
                .iter()
                .zip(&self.parts)
                .map(|(mesh, part)| {
                    Instance::new(mesh, transform, if part.tinted { tinted } else { plain })
                })
                .collect(),
        }
    }
}

/// Textures with a colour multiplied in: hair, which the source game
/// stores pale and tints per actor.
struct Tinted<'a> {
    inner: &'a MirTextures<'a>,
    tint: Option<[u8; 3]>,
}

impl foundry_render::TextureSource for Tinted<'_> {
    fn texture(&self, path: &str) -> Option<std::borrow::Cow<'_, foundry_render::Texture>> {
        let texture = self.inner.texture(path)?;
        let Some(tint) = self.tint else {
            return Some(texture);
        };
        let mut texture = texture.into_owned();
        for pixel in texture.rgba.as_chunks_mut::<4>().0 {
            for k in 0..3 {
                pixel[k] = (u16::from(pixel[k]) * u16::from(tint[k]) / 255) as u8;
            }
        }
        Some(std::borrow::Cow::Owned(texture))
    }
}

/// Geometry the source game hides until a limb comes off.
fn is_gore(primitive: &nif::Primitive) -> bool {
    primitive.name.to_ascii_lowercase().starts_with("meatcap")
        || primitive
            .material
            .textures
            .first()
            .is_some_and(|path| path.contains("\\gore\\"))
}

impl Shared<'_> {
    /// Whether the stage renders this actor: a human with a skeleton, both
    /// animations and every part converted.
    fn renderable(doc: &ActorDoc) -> Result<(), String> {
        if doc.kind != ActorKind::Npc {
            return Err("not a human: creatures are not rendered yet".to_owned());
        }
        if doc.skeleton.is_none() {
            return Err("no skeleton".to_owned());
        }
        if doc.animations.is_none() {
            return Err("no stand and walk animation".to_owned());
        }
        if doc.parts.is_empty() {
            return Err("no body parts".to_owned());
        }
        if let Some(part) = doc.parts.iter().find(|p| p.mesh.is_none()) {
            return Err(format!("part {} was not converted", part.path));
        }
        Ok(())
    }

    fn key(&self, doc: &ActorDoc) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.key_prefix);
        hasher.update(doc.appearance.as_bytes());
        hasher.update([0]);
        hasher.update(doc.skeleton.as_deref().unwrap_or("").as_bytes());
        hasher.update([0]);
        if let Some(animations) = &doc.animations {
            hasher.update(animations.stand.as_bytes());
            hasher.update([0]);
            hasher.update(animations.walk.as_bytes());
            hasher.update([0]);
        }
        hex(&hasher.finalize())
    }

    /// Loads the skeleton, the animations and the parts, with each part's
    /// skin texture override applied to its materials.
    fn body(&self, doc: &ActorDoc) -> Result<Body, String> {
        let skeleton = doc.skeleton.as_deref().ok_or("no skeleton")?;
        let animations = doc.animations.as_ref().ok_or("no animations")?;
        let skeleton = self
            .mir
            .skeleton(skeleton)
            .map_err(|e| format!("skeleton: {e}"))?;
        let stand = self
            .mir
            .animation(&animations.stand)
            .map_err(|e| format!("stand animation: {e}"))?;
        let walk = self
            .mir
            .animation(&animations.walk)
            .map_err(|e| format!("walk animation: {e}"))?;
        let mut parts = Vec::with_capacity(doc.parts.len());
        for part in &doc.parts {
            let hash = part.mesh.as_deref().ok_or("an unconverted part")?;
            let mut mesh = self
                .mir
                .mesh(hash)
                .map_err(|e| format!("part {}: {e}", part.path))?;
            if let Some(texture) = &part.texture {
                for primitive in &mut mesh.primitives {
                    match primitive.material.textures.first_mut() {
                        Some(diffuse) => diffuse.clone_from(texture),
                        None => primitive.material.textures.push(texture.clone()),
                    }
                }
            }
            let hair = part.role == mir::PartRole::Hair;
            // A hair file holds the hair twice: whole, and cut down to
            // sit under a hat. An actor whose hat hides the hair has no
            // hair part at all, so the whole one is the one to draw.
            mesh.primitives.retain(|primitive| {
                !is_gore(primitive) && !(hair && primitive.name.eq_ignore_ascii_case("hat"))
            });
            if !mesh.primitives.is_empty() {
                parts.push(Part { mesh, tinted: hair });
            }
        }
        if parts.is_empty() {
            return Err("no visible geometry".to_owned());
        }
        Ok(Body {
            head: skeleton.find(HEAD_BONE),
            rest: nif::Pose::rest(&skeleton),
            skeleton,
            stand,
            walk,
            parts,
            hair: doc.tint.hair,
        })
    }

    /// Renders one animation: every frame in all six directions. `walking`
    /// hands each frame's travel to the frame; a stand has none.
    #[allow(clippy::too_many_arguments)]
    fn animate(
        &self,
        body: &Body,
        animation: &nif::Animation,
        fps: u16,
        walking: bool,
        renderer: &mut dyn foundry_render::Renderer,
        textures: &MirTextures<'_>,
        missing: &mut std::collections::BTreeSet<String>,
    ) -> Result<(emit::CritterFrames, f32), String> {
        let times = frame_times(animation.duration, fps);
        let tinted = Tinted {
            inner: textures,
            tint: body.hair,
        };
        let mut directions: [Vec<emit::RenderedFrame>; 6] = Default::default();
        // Travel since the previous frame, in the root's own axes; frame 0
        // follows the cycle's last frame.
        let travel_at = |time: f32| nif::pose(&body.skeleton, animation, time).travel;
        let cycle = travel_at(animation.duration);
        let mut previous = travel_at(times[times.len() - 1]);
        previous = [previous[0] - cycle[0], previous[1] - cycle[1]];
        for &time in &times {
            let pose = nif::pose(&body.skeleton, animation, time);
            let step = if walking {
                [pose.travel[0] - previous[0], pose.travel[1] - previous[1]]
            } else {
                [0.0, 0.0]
            };
            previous = pose.travel;
            let posed = body.posed(&pose);
            for (direction, frames) in directions.iter_mut().enumerate() {
                // The walk's travel turns with the figure but is not
                // widened with it.
                let transform = mir::Transform {
                    position: [0.0; 3],
                    rotation: [0.0, 0.0, self.yaws[direction]],
                    scale: 1.0,
                }
                .matrix();
                let figure = figure_transform(self.yaws[direction], 1.0);
                let scene = body.scene(&posed, figure, textures, &tinted);
                let mut output = renderer
                    .render(&scene, Window::Fit)
                    .map_err(|e| format!("render: {e}"))?;
                lift(&mut output.rgba);
                missing.extend(output.missing_textures.iter().cloned());
                let pixels = quantize::quantize(
                    &output.rgba,
                    output.width,
                    output.height,
                    self.map,
                    Dither::None,
                )
                .map_err(|e| format!("quantize: {e}"))?;
                // The step, turned with the figure, on the screen.
                let ground = [
                    transform[0][0] * step[0] + transform[0][1] * step[1],
                    transform[1][0] * step[0] + transform[1][1] * step[1],
                    0.0,
                ];
                let travel = self
                    .projection
                    .screen(self.projection.tile_from_source([0.0; 3], ground));
                frames.push(emit::RenderedFrame {
                    origin: output.origin,
                    width: output.width,
                    height: output.height,
                    pixels,
                    travel,
                });
            }
        }
        let length = (cycle[0] * cycle[0] + cycle[1] * cycle[1]).sqrt();
        Ok((emit::CritterFrames { fps, directions }, length))
    }

    /// Renders and writes one sprite set. `Ok(true)` when it was rendered,
    /// `Ok(false)` when the cache already had it.
    fn build(
        &self,
        doc: &ActorDoc,
        key: &str,
        renderer: &mut dyn foundry_render::Renderer,
        textures: &MirTextures<'_>,
    ) -> Result<bool, String> {
        let dir = self.dir.join(key);
        let meta_path = dir.join("set.json");
        if meta_path.is_file() {
            return Ok(false);
        }
        let body = self.body(doc)?;
        let mut missing = std::collections::BTreeSet::new();
        let (stand, _) = self.animate(
            &body,
            &body.stand,
            STAND_FPS,
            false,
            renderer,
            textures,
            &mut missing,
        )?;
        let (walk, cycle) = self.animate(
            &body,
            &body.walk,
            WALK_FPS,
            true,
            renderer,
            textures,
            &mut missing,
        )?;
        let stand_spec = emit::critter_frm(&stand, 0).map_err(|e| format!("stand: {e}"))?;
        let walk_spec = emit::critter_frm(&walk, 0).map_err(|e| format!("walk: {e}"))?;
        let still = emit::still_frm(&stand_spec).map_err(|e| format!("still: {e}"))?;
        let encode = |spec: &emit::FrmSpec, what: &str| -> Result<Vec<u8>, String> {
            let bytes = emit::FormatWriters::frm(&emit::LocalWriters, spec)
                .map_err(|e| format!("{what}: {e}"))?;
            // Never leave a file the engine's own parser would refuse.
            FrmArchive::parse(&bytes).map_err(|e| format!("{what} does not parse back: {e}"))?;
            Ok(bytes)
        };
        let mut files: Vec<(String, Vec<u8>)> = vec![
            (STAND.to_owned(), encode(&stand_spec, "stand")?),
            (WALK.to_owned(), encode(&walk_spec, "walk")?),
        ];
        let still_bytes = encode(&still, "still")?;
        for suffix in PLACEHOLDERS {
            files.push((suffix.to_owned(), still_bytes.clone()));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for (suffix, bytes) in &files {
            let path = dir.join(format!("{suffix}.frm"));
            std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let meta = CritterSetMeta {
            appearance: doc.appearance.clone(),
            label: doc
                .editor_id
                .clone()
                .unwrap_or_else(|| doc.form.to_string()),
            stand_frames: stand.directions[0].len(),
            walk_frames: walk.directions[0].len(),
            walk_cycle_units: cycle,
            animations: files.iter().map(|(suffix, _)| suffix.clone()).collect(),
            missing_textures: missing.into_iter().collect(),
        };
        let text = serde_json::to_string_pretty(&meta).expect("plain data");
        std::fs::write(&meta_path, text + "\n")
            .map_err(|e| format!("{}: {e}", meta_path.display()))?;
        Ok(true)
    }
}

/// Renders a sprite set for every distinct human appearance in the slice
/// to `<out>/critters/`.
pub fn critters(inputs: &Inputs) -> Result<CrittersSummary, ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("critters", e);
    let mir = Mir::open(&inputs.mir_dir()).map_err(|e| stage(&e))?;
    let palette_bytes = inputs.f2_file("color.pal")?;
    let palette = inputs.palette()?;
    let map = PaletteMap::try_new(&palette).map_err(|e| stage(&e))?;
    let dir = inputs.critters_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let yaws = direction_yaws(inputs)?;

    // The render path is part of the key, as for sprites.
    let backend = match inputs.backend {
        Backend::Auto => foundry_render::renderer(Backend::Auto, &inputs.calibration)
            .map_err(|e| stage(&e))?
            .backend(),
        other => other,
    };
    let mut prefix = Sha256::new();
    prefix.update(STAGE_VERSION.as_bytes());
    prefix.update(
        inputs
            .calibration
            .to_toml()
            .map_err(|e| stage(&e))?
            .as_bytes(),
    );
    prefix.update(Sha256::digest(&palette_bytes));
    prefix.update(crate::sprites::backend_name(backend).as_bytes());
    prefix.update(STAND_FPS.to_le_bytes());
    prefix.update(WALK_FPS.to_le_bytes());
    for yaw in yaws {
        prefix.update(yaw.to_le_bytes());
    }
    let shared = Shared {
        mir: &mir,
        map: &map,
        projection: Projection::new(&inputs.calibration).map_err(|e| stage(&e))?,
        yaws,
        key_prefix: prefix.finalize().to_vec(),
        dir: dir.clone(),
    };

    // Every actor document, in form order; one job per distinct set.
    let mut docs = Vec::new();
    for entry in mir.actors() {
        if let Some(doc) = mir.actor(&entry.form).map_err(|e| stage(&e))? {
            docs.push(doc);
        }
    }
    docs.sort_by(|a, b| a.form.cmp(&b.form));
    let mut keys: Vec<Result<String, String>> = Vec::with_capacity(docs.len());
    let mut jobs: BTreeMap<String, usize> = BTreeMap::new();
    for (index, doc) in docs.iter().enumerate() {
        let key = Shared::renderable(doc).map(|()| shared.key(doc));
        if let Ok(key) = &key {
            jobs.entry(key.clone()).or_insert(index);
        }
        keys.push(key);
    }
    let jobs: Vec<(String, usize)> = jobs.into_iter().collect();

    let workers = if backend == Backend::Gpu {
        1
    } else {
        inputs.jobs.clamp(1, jobs.len().max(1))
    };
    let chunk = jobs.len().div_ceil(workers).max(1);
    let docs_ref = &docs;
    let shared_ref = &shared;
    let run = |slice: &[(String, usize)]| -> Result<Vec<Result<bool, String>>, String> {
        let mut renderer =
            foundry_render::renderer(backend, &inputs.calibration).map_err(|e| e.to_string())?;
        let textures = MirTextures {
            mir: &mir,
            cache: RefCell::new(BTreeMap::new()),
        };
        Ok(slice
            .iter()
            .map(|(key, index)| {
                shared_ref.build(&docs_ref[*index], key, renderer.as_mut(), &textures)
            })
            .collect())
    };
    let results: Vec<Result<bool, String>> = std::thread::scope(|scope| {
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

    let mut summary = CrittersSummary {
        actors: docs.len(),
        sets: jobs.len(),
        ..CrittersSummary::default()
    };
    let mut outcome: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    for ((key, _), result) in jobs.iter().zip(&results) {
        match result {
            Ok(true) => summary.rendered += 1,
            Ok(false) => summary.reused += 1,
            Err(_) => summary.failed += 1,
        }
        outcome.insert(key, result.as_ref().err().map(String::as_str));
    }
    let index: Vec<CritterEntry> = docs
        .iter()
        .zip(&keys)
        .map(|(doc, key)| {
            let (set, error) = match key {
                Ok(key) => match outcome.get(key.as_str()).copied().flatten() {
                    Some(reason) => (None, Some(reason.to_owned())),
                    None => (Some(key.clone()), None),
                },
                Err(reason) => (None, Some(reason.clone())),
            };
            CritterEntry {
                actor: doc.form.to_string(),
                editor_id: doc.editor_id.clone(),
                name: doc.name.clone(),
                female: doc.female,
                health: doc.health,
                set,
                error,
            }
        })
        .collect();
    let index_path = dir.join("index.json");
    let text = serde_json::to_string_pretty(&index).expect("plain data");
    std::fs::write(&index_path, text + "\n").map_err(io(&index_path))?;
    Ok(summary)
}

/// Reads `index.json` back; empty when the stage has not run.
pub fn read_index(inputs: &Inputs) -> Result<Vec<CritterEntry>, ConvertError> {
    let path = inputs.critters_dir().join("index.json");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(io(&path))?;
    serde_json::from_str(&text).map_err(|e| ConvertError::stage("critters", e))
}

/// The animation files of sprite set `key`, by suffix.
pub fn read_set(inputs: &Inputs, key: &str) -> Result<BTreeMap<String, Vec<u8>>, ConvertError> {
    let dir = inputs.critters_dir().join(key);
    let meta_path = dir.join("set.json");
    let text = std::fs::read_to_string(&meta_path).map_err(io(&meta_path))?;
    let meta: CritterSetMeta =
        serde_json::from_str(&text).map_err(|e| ConvertError::stage("critters", e))?;
    let mut files = BTreeMap::new();
    for suffix in meta.animations {
        let path = dir.join(format!("{suffix}.frm"));
        files.insert(suffix, std::fs::read(&path).map_err(io(&path))?);
    }
    Ok(files)
}

/// A large look at one actor, for tuning: its stand pose at `scale` times
/// the map's size, facing each of `directions`, side by side on a mid-grey
/// ground, as RGBA. The actor is named by editor id.
pub fn preview(
    inputs: &Inputs,
    editor_id: &str,
    scale: f32,
    directions: &[usize],
) -> Result<(Vec<u8>, u32, u32), ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("critters", e);
    let mir = Mir::open(&inputs.mir_dir()).map_err(|e| stage(&e))?;
    let palette = inputs.palette()?;
    let map = PaletteMap::try_new(&palette).map_err(|e| stage(&e))?;
    let mut found = None;
    for entry in mir.actors() {
        if let Some(doc) = mir.actor(&entry.form).map_err(|e| stage(&e))?
            && doc
                .editor_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(editor_id))
        {
            found = Some(doc);
            break;
        }
    }
    let doc = found.ok_or_else(|| stage(&format!("no actor named {editor_id}")))?;
    Shared::renderable(&doc).map_err(|e| stage(&e))?;
    let shared = Shared {
        mir: &mir,
        map: &map,
        projection: Projection::new(&inputs.calibration).map_err(|e| stage(&e))?,
        yaws: direction_yaws(inputs)?,
        key_prefix: Vec::new(),
        dir: inputs.critters_dir(),
    };
    let body = shared.body(&doc).map_err(|e| stage(&e))?;
    let pose = nif::pose(&body.skeleton, &body.stand, 0.0);
    let posed = body.posed(&pose);
    let textures = MirTextures {
        mir: &mir,
        cache: RefCell::new(BTreeMap::new()),
    };
    let tinted = Tinted {
        inner: &textures,
        tint: body.hair,
    };
    let mut renderer =
        foundry_render::renderer(inputs.backend, &inputs.calibration).map_err(|e| stage(&e))?;
    let mut outputs = Vec::new();
    for &direction in directions {
        let yaw = *shared
            .yaws
            .get(direction)
            .ok_or_else(|| stage(&format!("no hex direction {direction}")))?;
        let scene = body.scene(&posed, figure_transform(yaw, scale), &textures, &tinted);
        let mut output = renderer
            .render(&scene, Window::Fit)
            .map_err(|e| stage(&e))?;
        lift(&mut output.rgba);
        outputs.push(output);
    }
    let width: u32 = outputs.iter().map(|o| o.width + 8).sum();
    let height = outputs.iter().map(|o| o.height).max().unwrap_or(1);
    let mut rgba = vec![128u8; (width * height * 4) as usize];
    for alpha in rgba.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    let mut left = 4u32;
    for output in &outputs {
        let top = height - output.height;
        for y in 0..output.height {
            for x in 0..output.width {
                let from = ((y * output.width + x) * 4) as usize;
                if output.rgba[from + 3] == 0 {
                    continue;
                }
                let to = (((top + y) * width + left + x) * 4) as usize;
                rgba[to..to + 3].copy_from_slice(&output.rgba[from..from + 3]);
            }
        }
        left += output.width + 8;
    }
    Ok((rgba, width, height))
}
