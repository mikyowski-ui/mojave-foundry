//! The critter stage: every distinct human appearance among the slice's
//! actors is posed frame by frame, rendered in the six hex directions,
//! quantized to the Fallout 2 palette and written as one sprite set.
//!
//! Output, under `<out>/critters/`:
//!
//! - `<key>/<suffix>.frm`, one per animation, and `<key>/set.json`, written
//!   last, which marks the set complete (the incremental cache). `<key>` is
//!   the content hash of everything that determines the set.
//! - `anims/<hash>.frm`: every rendered animation by its own content key,
//!   so a set that differs from a cached one in one animation renders only
//!   that one (levels [`AnimationLevel::Combat`] and up).
//! - `index.json`: one entry per actor document, sorted by form.
//! - `report.md` (levels above minimal): one line per set, which
//!   animations are real and which are placeholders.
//!
//! A frame is the actor's parts skinned to one pose of the skeleton and
//! turned to face a hex direction. Root motion is handled per kind:
//!
//! | Kind | Motion | Frames a second |
//! |---|---|---|
//! | stand (`aa`, armed `<w>a`) | none | [`STAND_FPS`] |
//! | walk, run (`ab`, `at`, armed `<w>b`) | a cycle: travel handed to `emit`, which turns it into offsets along the facing's hex step | [`WALK_FPS`] |
//! | dodge, hit, punch, kick, reach, weapon actions | in place: the root is held | [`ACTION_FPS`] |
//! | falls (`ba`, `bb`) and get-ups (`cj`, `ch`) | baked: the body moves in the pixels, offsets stay zero | [`ACTION_FPS`] |
//! | corpses (`ra`, `rb`) | one still, the last fall frame | - |
//!
//! [`AnimationLevel::Minimal`] is the first wave's output, byte for byte:
//! a stand, a walk and nine stills. The other levels resolve the
//! animations the extract found (`mir::human_animation_slots`), fill what
//! is still required with stills (`emit::AnimationSet::fill_placeholders`)
//! and say which is which.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use emit::{Anim, AnimationSet, AttackMode, Need, WeaponCode};
use foundry_render::{Backend, Instance, Projection, Scene, Window};
use mir::{ActorDoc, ActorKind, Mir};
use quantize::{Dither, PaletteMap};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vault_formats::frm::FrmArchive;

use crate::config::{ConvertError, Inputs};
use crate::sprites::MirTextures;

/// Bumped whenever the stage's output for the same inputs would change at
/// [`AnimationLevel::Minimal`]. Kept at the first wave's value so that the
/// minimal level writes what it always wrote.
const STAGE_VERSION: &str = "critters-3";
/// The same for every other level.
const ANIMATED_STAGE_VERSION: &str = "critters-5";
/// The version of `set.json` written by the levels above minimal. A set
/// written before has no `schema` field (it reads as 0).
pub const SET_SCHEMA: u32 = 2;
/// Frames a second of the stand; the idle is slow, and a set's size is
/// mostly its stand.
pub const STAND_FPS: u16 = 5;
/// Frames a second of the walk and the run: the engine's default rate.
pub const WALK_FPS: u16 = 10;
/// Frames a second of every one-shot animation (attacks, hits, falls).
pub const ACTION_FPS: u16 = 10;
/// How much wider than life a figure is drawn, about its own upright axis.
/// Fallout 2's figures are stocky; at the map's scale a true human reads
/// thin beside them (owner playtest, 2026-11-20).
pub const GIRTH: f32 = 1.15;
/// Gamma applied to a rendered figure before it is quantized; below 1
/// lifts the mid-tones. The source's outfits are dark, and at this size
/// they read as silhouettes (same playtest).
pub const LIFT_GAMMA: f32 = 0.78;
/// The stand's file suffix.
pub const STAND: &str = "aa";
/// The walk's file suffix.
pub const WALK: &str = "ab";
/// Animations a fight asks for, each one still frame of the stand at the
/// minimal level: hit, the two falls, the two get-ups, the two corpses,
/// punch and kick.
pub const PLACEHOLDERS: [&str; 9] = ["ao", "ba", "bb", "ch", "cj", "ra", "rb", "aq", "ar"];
/// The longest a get-up is allowed to take, in frames: the reversed fall
/// is thinned to it.
const GET_UP_FRAMES: usize = 12;
/// Seconds of a synthesized point (raise to aim) or unpoint.
const POINT_SECONDS: f32 = 0.3;
/// Seconds of automatic fire: the source's short fire loop repeated.
const BURST_SECONDS: f32 = 0.5;
/// The bones of a weapon in hand, in order of preference.
const WEAPON_BONES: [&str; 2] = ["Weapon", "Bip01 R Hand"];
/// The bone an upper-body overlay (aim over a walk) starts at.
const UPPER_BODY: &str = "Bip01 Spine1";
/// The hand a weapon is in while it is drawn.
const HAND_BONE: &str = "Bip01 R Hand";

/// How many animations a sprite set carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationLevel {
    /// The first wave: a real stand and walk, nine stills of the stand.
    /// Byte-identical to the output before the levels existed.
    Minimal,
    /// What a fight needs: every unarmed animation the source has, the
    /// armed set of the weapon the actor carries, stills for the rest of
    /// `emit::combat_requirements`.
    #[default]
    Combat,
    /// Combat, and the default pistol and rifle sets for every actor.
    Full,
}

impl std::str::FromStr for AnimationLevel {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "minimal" => Ok(Self::Minimal),
            "combat" => Ok(Self::Combat),
            "full" => Ok(Self::Full),
            other => Err(format!("{other}: expected minimal, combat or full")),
        }
    }
}

impl AnimationLevel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Combat => "combat",
            Self::Full => "full",
        }
    }
}

/// Where the falls and corpses come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeathSource {
    /// The [`DeathSampler`] (the `ragdoll` crate by default); the game's
    /// knock-down animations when it fails.
    #[default]
    Ragdoll,
    /// The game's knock-down animations only.
    Game,
}

/// How a figure is drawn: the owner's readability knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FigureLook {
    /// Horizontal widening about the upright axis ([`GIRTH`]).
    pub girth: f32,
    /// Gamma lift before quantizing ([`LIFT_GAMMA`]).
    pub lift_gamma: f32,
    /// A tone curve applied after the lift; identity by default.
    pub tone: foundry_render::Tone,
    /// A rim around or inside the silhouette after quantizing; none by
    /// default (the owner has not chosen yet).
    pub outline: Option<quantize::RimOptions>,
}

impl Default for FigureLook {
    fn default() -> Self {
        FigureLook {
            girth: GIRTH,
            lift_gamma: LIFT_GAMMA,
            tone: foundry_render::Tone::IDENTITY,
            outline: None,
        }
    }
}

impl FigureLook {
    /// The readable variant: the figure tone of
    /// `foundry_render::ClassTones::readable()` and a dark outer rim
    /// (linear light × 0.45), for the before/after pictures.
    pub fn readable() -> Self {
        FigureLook {
            tone: foundry_render::ClassTones::readable().figures,
            outline: Some(quantize::RimOptions::outer(0.45)),
            ..FigureLook::default()
        }
    }

    fn is_default(&self) -> bool {
        *self == FigureLook::default()
    }

    fn key(&self) -> String {
        let rim = self.outline.map_or_else(
            || "-".to_owned(),
            |r| format!("{}:{}:{}", r.factor, r.width, r.outer),
        );
        let t = self.tone;
        format!(
            "girth {} lift {} tone {} {} {} {} rim {rim}",
            self.girth, self.lift_gamma, t.gain, t.gamma, t.saturation, t.contrast
        )
    }
}

/// How a walk or run cycle's travel meets the hex grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stride {
    /// The travel the animation has, not widened with the figure (the
    /// first wave's rule): the feet, drawn [`FigureLook::girth`] wider,
    /// slide back by the difference. The default: measured on the real
    /// walks, [`Stride::Matched`] gains under 1 pixel a cycle on the walk
    /// and loses 2 on the run (the source's own feet move while planted),
    /// so the first wave's rule stays.
    #[default]
    Exact,
    /// The travel widened with the figure, so a foot planted in the
    /// source keeps its place on screen; a cycle covers `girth` times the
    /// source's ground.
    Matched,
    /// [`Stride::Matched`] scaled so a cycle covers a whole number of
    /// hexes (at least one): cycles start on hex centres, the feet slide
    /// by the difference.
    WholeHexes,
}

/// A fall the death sampler is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fall {
    /// Onto the back (`ba`, corpse `ra`).
    Back,
    /// Onto the front (`bb`, corpse `rb`).
    Front,
}

/// Physics falls: from a standing pose to a pose at rest on flat ground.
pub trait DeathSampler: Send + Sync {
    /// Poses in the skeleton's root space, `sample_rate` a second, the
    /// first `start`, the last at rest; the body moves away from where it
    /// stood (the travel is in the poses, not in [`nif::Pose::travel`]).
    fn fall(
        &self,
        skeleton: &nif::Skeleton,
        start: &nif::Pose,
        fall: Fall,
        seed: u64,
        sample_rate: f32,
    ) -> Result<Vec<nif::Pose>, String>;
    /// Part of every cache key the falls go into.
    fn name(&self) -> String;
}

/// [`DeathSampler`] by the `ragdoll` crate: the humanoid spec, the fall
/// presets, at most 3.5 s.
#[derive(Debug, Clone, Copy, Default)]
pub struct RagdollSampler;

impl DeathSampler for RagdollSampler {
    fn fall(
        &self,
        skeleton: &nif::Skeleton,
        start: &nif::Pose,
        fall: Fall,
        seed: u64,
        sample_rate: f32,
    ) -> Result<Vec<nif::Pose>, String> {
        let spec = ragdoll::RagdollSpec::humanoid();
        let preset = match fall {
            Fall::Back => ragdoll::Preset::FallBackward,
            Fall::Front => ragdoll::Preset::FallForward,
        };
        let impulse = ragdoll::Impulse::preset(preset);
        let clip = ragdoll::simulate(skeleton, start, &spec, &impulse, seed, 3.5, sample_rate)
            .map_err(|e| e.to_string())?;
        Ok(clip.poses)
    }

    fn name(&self) -> String {
        "ragdoll humanoid 3.5s".to_owned()
    }
}

/// Everything the stage takes beyond [`Inputs`]. The default is what
/// `all` does.
#[derive(Clone)]
pub struct CritterOptions {
    pub level: AnimationLevel,
    pub look: FigureLook,
    pub deaths: DeathSource,
    pub stride: Stride,
    /// The Fallout 2 weapon code and attack modes of a carried weapon, by
    /// the weapon's form key (`mir::FormKey` text), as the rebalance
    /// classifies it. A weapon not listed takes [`family_weapon`] of its
    /// animation family.
    pub weapon_codes: BTreeMap<String, (WeaponCode, [AttackMode; 2])>,
    /// Render only the actors with these editor ids (any case); empty
    /// renders all. The others are in the index with a reason.
    pub only: Vec<String>,
    /// The falls; [`RagdollSampler`] by default.
    pub sampler: Arc<dyn DeathSampler>,
}

impl Default for CritterOptions {
    fn default() -> Self {
        CritterOptions {
            level: AnimationLevel::default(),
            look: FigureLook::default(),
            deaths: DeathSource::default(),
            stride: Stride::default(),
            weapon_codes: BTreeMap::new(),
            only: Vec::new(),
            sampler: Arc::new(RagdollSampler),
        }
    }
}

impl std::fmt::Debug for CritterOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CritterOptions")
            .field("level", &self.level)
            .field("look", &self.look)
            .field("deaths", &self.deaths)
            .field("stride", &self.stride)
            .field("weapon_codes", &self.weapon_codes)
            .field("only", &self.only)
            .field("sampler", &self.sampler.name())
            .finish()
    }
}

/// The default Fallout 2 weapon code and modes of a source animation
/// family (`mir::weapon_family`), the rebalance's choices for the
/// family's usual class: pistols single shot, automatic rifles single
/// and burst, rifles (Fallout 2's rifle letter, `emit`'s
/// [`WeaponCode::Shotgun`]) single, heavy weapons burst, one-hand melee a
/// club's swing, two-hand melee a sledge's swing. `None` for hand to
/// hand, thrown weapons and mines.
pub fn family_weapon(family: &str) -> Option<(WeaponCode, [AttackMode; 2])> {
    use AttackMode as M;
    Some(match family {
        "1hm" => (WeaponCode::Club, [M::Swing, M::None]),
        "2hm" => (WeaponCode::Hammer, [M::Swing, M::None]),
        "1hp" => (WeaponCode::Pistol, [M::Single, M::None]),
        "2ha" => (WeaponCode::Smg, [M::Single, M::Burst]),
        "2hr" => (WeaponCode::Shotgun, [M::Single, M::None]),
        "2hh" => (WeaponCode::Minigun, [M::Burst, M::None]),
        "2hl" => (WeaponCode::Launcher, [M::Single, M::None]),
        _ => return None,
    })
}

/// What `<key>/set.json` records about one sprite set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CritterSetMeta {
    /// [`SET_SCHEMA`]; absent (0) in a set of the minimal level or one
    /// written before levels existed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub schema: u32,
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
    /// The level the set was made at; absent for minimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<AnimationLevel>,
    /// Per real animation: where it came from, its frames and action
    /// frame. Empty for minimal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub real: BTreeMap<String, AnimationNote>,
    /// Animations that are a still of the stand. Empty for minimal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placeholders: Vec<String>,
    /// The weapon codes with an armed set, and the model drawn in hand.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub weapons: BTreeMap<String, String>,
    /// Bytes of all the set's files.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub bytes: u64,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

/// One real animation of a set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationNote {
    /// What it was made from: a source slot and file stem, `ragdoll`, or
    /// `reversed ba`.
    pub source: String,
    pub frames: usize,
    pub fps: u16,
    /// The FRM's action frame.
    pub action_frame: u16,
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
    /// One line per set rendered by this run: real and placeholder
    /// animations, frames, bytes and wall-clock seconds. Not written
    /// anywhere (the seconds differ run to run); `report.md` has the rest.
    pub lines: Vec<String>,
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

/// The transform of a figure facing `yaw` at `scale`: widened by `girth`
/// about its upright axis, then turned.
fn figure_transform_with(yaw: f32, scale: f32, girth: f32) -> [[f32; 4]; 3] {
    let mut matrix = mir::Transform {
        position: [0.0; 3],
        rotation: [0.0, 0.0, yaw],
        scale,
    }
    .matrix();
    for row in &mut matrix {
        row[0] *= girth;
        row[1] *= girth;
    }
    matrix
}

/// [`figure_transform_with`] at [`GIRTH`].
fn figure_transform(yaw: f32, scale: f32) -> [[f32; 4]; 3] {
    figure_transform_with(yaw, scale, GIRTH)
}

/// Lifts the mid-tones of every opaque pixel by `gamma`.
fn lift_with(rgba: &mut [u8], gamma: f32) {
    let table: [u8; 256] = std::array::from_fn(|v| {
        ((v as f32 / 255.0).powf(gamma) * 255.0)
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

/// Lifts the mid-tones of every opaque pixel by [`LIFT_GAMMA`].
fn lift(rgba: &mut [u8]) {
    lift_with(rgba, LIFT_GAMMA);
}

/// The animation times of `frames` evenly spaced frames over `duration`.
fn frame_times(duration: f32, fps: u16) -> Vec<f32> {
    let count = ((duration * f32::from(fps)).round() as usize).max(1);
    (0..count)
        .map(|k| duration * k as f32 / count as f32)
        .collect()
}

/// The times of a one-shot animation: its first and its last pose
/// included, about `fps` apart.
fn once_times(duration: f32, fps: u16) -> Vec<f32> {
    let duration = if duration.is_finite() {
        duration.max(0.0)
    } else {
        0.0
    };
    let count = ((duration * f32::from(fps)).round() as usize + 1).max(2);
    (0..count)
        .map(|k| duration * k as f32 / (count - 1) as f32)
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

/// Applies a 3 × 4 matrix to a mesh's positions and normals.
fn transform_mesh(mesh: &mut nif::StaticMesh, matrix: &[[f32; 4]; 3]) {
    for primitive in &mut mesh.primitives {
        for p in &mut primitive.positions {
            *p = [0, 1, 2].map(|r| {
                matrix[r][0] * p[0] + matrix[r][1] * p[1] + matrix[r][2] * p[2] + matrix[r][3]
            });
        }
        for n in primitive.normals.iter_mut().flatten() {
            let v =
                [0, 1, 2].map(|r| matrix[r][0] * n[0] + matrix[r][1] * n[1] + matrix[r][2] * n[2]);
            let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if length > 0.0 {
                *n = v.map(|c| c / length);
            }
        }
    }
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
                let mut mesh = part.mesh.clone();
                transform_mesh(&mut mesh, &matrix);
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

// ---------------------------------------------------------------------
// Clips: the poses of one animation, before rendering.

/// How a clip's root motion reaches the FRM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipMotion {
    /// A loop that walks: travel per frame along the hex step.
    Cycle,
    /// Root held; no travel.
    InPlace,
    /// The body moves in the pixels ([`emit::Motion::Baked`]).
    Baked,
}

/// One pose to render.
#[derive(Debug, Clone)]
struct Sample {
    pose: nif::Pose,
    /// Draw the weapon in hand.
    weapon: bool,
}

/// One animation's poses and how to write them.
struct Clip {
    anim: Anim,
    weapon: WeaponCode,
    fps: u16,
    motion: ClipMotion,
    samples: Vec<Sample>,
    /// The contact sample (an attack's `Hit`), for the action frame.
    contact: Option<usize>,
    /// Raise the figure so no vertex is below the ground.
    clamp_ground: bool,
    /// What it was made from, for the report.
    source: String,
    /// Everything that determines its frames beyond the body and the
    /// look: part of its cache key.
    key: String,
    /// The weapon model drawn while `Sample::weapon` is set.
    weapon_mesh: Option<String>,
    /// The root travel of one whole cycle (walks and runs), root axes.
    cycle: [f32; 2],
}

/// The nearest sample index to `time` among `times`.
fn nearest(times: &[f32], time: f32) -> Option<usize> {
    times
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1 - time)
                .abs()
                .partial_cmp(&(b.1 - time).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// A pose with its travel moved into the matrices (root-space X and Y),
/// so the body is drawn where the root has gone.
fn baked(mut pose: nif::Pose) -> nif::Pose {
    let [x, y] = pose.travel;
    for m in &mut pose.world {
        m[0][3] += x;
        m[1][3] += y;
    }
    pose.travel = [0.0; 2];
    pose
}

/// The animations of one actor document and the body's skeleton.
struct Sources<'a> {
    mir: &'a Mir,
    hashes: &'a BTreeMap<String, String>,
    cache: RefCell<BTreeMap<String, Option<Arc<nif::Animation>>>>,
}

impl Sources<'_> {
    fn get(&self, slot: &str) -> Option<(Arc<nif::Animation>, String)> {
        let hash = self.hashes.get(slot)?;
        let mut cache = self.cache.borrow_mut();
        let entry = cache
            .entry(slot.to_owned())
            .or_insert_with(|| self.mir.animation(hash).ok().map(Arc::new));
        entry.clone().map(|a| (a, hash.clone()))
    }
}

/// A short name of an animation for the report: the sequence name.
fn source_name(slot: &str, animation: &nif::Animation) -> String {
    if animation.name.is_empty() {
        slot.to_owned()
    } else {
        format!("{slot} ({})", animation.name)
    }
}

/// Everything about one armed set.
#[derive(Debug, Clone)]
struct ArmedSet {
    code: WeaponCode,
    modes: [AttackMode; 2],
    family: String,
    /// Mesh hash of the model drawn in hand.
    mesh: Option<String>,
    path: String,
}

/// Builds the clips of a set. `None` entries of the result are
/// animations the source lacks.
struct ClipMaker<'a> {
    body: &'a Body,
    sources: Sources<'a>,
    options: &'a CritterOptions,
    seed: u64,
    notes: RefCell<Vec<String>>,
}

impl ClipMaker<'_> {
    fn skeleton(&self) -> &nif::Skeleton {
        &self.body.skeleton
    }

    /// A looping animation: stand-like (`InPlace`) or walking (`Cycle`).
    #[allow(clippy::too_many_arguments)]
    fn looping(
        &self,
        anim: Anim,
        weapon: WeaponCode,
        animation: &nif::Animation,
        hash: &str,
        fps: u16,
        walking: bool,
        source: String,
    ) -> Clip {
        let times = frame_times(animation.duration, fps);
        let samples = times
            .iter()
            .map(|&t| Sample {
                pose: nif::pose(self.skeleton(), animation, t),
                weapon: weapon != WeaponCode::Unarmed,
            })
            .collect();
        let cycle = nif::pose(self.skeleton(), animation, animation.duration).travel;
        Clip {
            anim,
            weapon,
            fps,
            motion: if walking {
                ClipMotion::Cycle
            } else {
                ClipMotion::InPlace
            },
            samples,
            contact: None,
            clamp_ground: false,
            source,
            key: format!("loop {hash} {fps} {walking}"),
            weapon_mesh: None,
            cycle,
        }
    }

    /// A one-shot in place, its contact at the `Hit` key or `contact`
    /// (a fraction of the length) without one.
    fn once(
        &self,
        anim: Anim,
        weapon: WeaponCode,
        slot: &str,
        contact: Option<f32>,
    ) -> Option<Clip> {
        let (animation, hash) = self.sources.get(slot)?;
        let times = once_times(animation.duration, ACTION_FPS);
        let samples = times
            .iter()
            .map(|&t| Sample {
                pose: nif::pose(self.skeleton(), &animation, t),
                weapon: weapon != WeaponCode::Unarmed,
            })
            .collect();
        let hit = animation
            .text_key_time("hit")
            .or_else(|| contact.map(|f| f * animation.duration));
        Some(Clip {
            anim,
            weapon,
            fps: ACTION_FPS,
            motion: ClipMotion::InPlace,
            samples,
            contact: hit.and_then(|t| nearest(&times, t)),
            clamp_ground: false,
            source: source_name(slot, &animation),
            key: format!("once {hash} {contact:?}"),
            weapon_mesh: None,
            cycle: [0.0; 2],
        })
    }

    /// The armed stand: the family's aim, looping.
    fn armed_stand(&self, set: &ArmedSet) -> Option<Clip> {
        let slot = format!("{}.stand", set.family);
        let (animation, hash) = self.sources.get(&slot)?;
        Some(self.looping(
            Anim::Stand,
            set.code,
            &animation,
            &hash,
            STAND_FPS,
            false,
            source_name(&slot, &animation),
        ))
    }

    /// The armed walk: the family's own forward walk, or the legs of the
    /// unarmed walk under the upper body of the family's aim (Fallout 3
    /// has armed walks for one-hand weapons only).
    fn armed_walk(&self, set: &ArmedSet, walk_hash: &str) -> Option<Clip> {
        let slot = format!("{}.walk", set.family);
        if let Some((animation, hash)) = self.sources.get(&slot) {
            return Some(self.looping(
                Anim::Walk,
                set.code,
                &animation,
                &hash,
                WALK_FPS,
                true,
                source_name(&slot, &animation),
            ));
        }
        let (aim, aim_hash) = self.sources.get(&format!("{}.stand", set.family))?;
        let walk = &self.body.walk;
        let mask = nif::subtree_mask(self.skeleton(), UPPER_BODY);
        let times = frame_times(walk.duration, WALK_FPS);
        let samples = times
            .iter()
            .map(|&t| Sample {
                pose: nif::compose(
                    self.skeleton(),
                    &nif::layer(
                        &nif::local_pose(self.skeleton(), walk, t),
                        &nif::local_pose(self.skeleton(), &aim, t),
                        &mask,
                    ),
                ),
                weapon: true,
            })
            .collect();
        let cycle = nif::pose(self.skeleton(), walk, walk.duration).travel;
        Some(Clip {
            anim: Anim::Walk,
            weapon: set.code,
            fps: WALK_FPS,
            motion: ClipMotion::Cycle,
            samples,
            contact: None,
            clamp_ground: false,
            source: format!("walk under {}.stand", set.family),
            key: format!("layer {walk_hash} {aim_hash} {UPPER_BODY}"),
            weapon_mesh: None,
            cycle,
        })
    }

    /// The armed dodge: the family's block, or the unarmed dodge under
    /// the aim's upper body.
    fn armed_dodge(&self, set: &ArmedSet) -> Option<Clip> {
        if let Some(clip) = self.once(
            Anim::Dodge,
            set.code,
            &format!("{}.block", set.family),
            Some(0.5),
        ) {
            return Some(clip);
        }
        let (dodge, dodge_hash) = self.sources.get("dodge")?;
        let (aim, aim_hash) = self.sources.get(&format!("{}.stand", set.family))?;
        let mask = nif::subtree_mask(self.skeleton(), UPPER_BODY);
        let times = once_times(dodge.duration, ACTION_FPS);
        let samples = times
            .iter()
            .map(|&t| Sample {
                pose: nif::compose(
                    self.skeleton(),
                    &nif::layer(
                        &nif::local_pose(self.skeleton(), &dodge, t),
                        &nif::local_pose(self.skeleton(), &aim, t),
                        &mask,
                    ),
                ),
                weapon: true,
            })
            .collect();
        Some(Clip {
            anim: Anim::Dodge,
            weapon: set.code,
            fps: ACTION_FPS,
            motion: ClipMotion::InPlace,
            samples,
            contact: nearest(&times, dodge.duration * 0.5),
            clamp_ground: false,
            source: format!("dodge under {}.stand", set.family),
            key: format!("layer-once {dodge_hash} {aim_hash} {UPPER_BODY}"),
            weapon_mesh: None,
            cycle: [0.0; 2],
        })
    }

    /// Take out or put away: the weapon shows in the hand from (take out)
    /// or until (put away) the source's parent switch to the hand.
    fn equip(&self, set: &ArmedSet, take_out: bool) -> Option<Clip> {
        let slot = format!(
            "{}.{}",
            set.family,
            if take_out { "equip" } else { "unequip" }
        );
        let anim = if take_out {
            Anim::TakeOut
        } else {
            Anim::PutAway
        };
        let mut clip = self.once(anim, set.code, &slot, Some(0.5))?;
        let (animation, _) = self.sources.get(&slot)?;
        let times = once_times(animation.duration, ACTION_FPS);
        // The switch: the source's `Attach` (take out) or `Detach` (put
        // away) key; else its parent switch into or out of the hand; half
        // way without either.
        let switch = if take_out {
            animation
                .text_key_time("attach")
                .or_else(|| animation.parent_switch_time(HAND_BONE))
        } else {
            animation.text_key_time("detach").or_else(|| {
                animation.text_keys.iter().find_map(|(t, text)| {
                    let (head, rest) = text.trim().split_once(':')?;
                    (head.trim().eq_ignore_ascii_case("prn")
                        && !rest.trim().eq_ignore_ascii_case(HAND_BONE))
                    .then_some(*t)
                })
            })
        }
        .unwrap_or(animation.duration * 0.5);
        for (sample, &t) in clip.samples.iter_mut().zip(&times) {
            sample.weapon = if take_out { t >= switch } else { t < switch };
        }
        clip.contact = nearest(&times, switch);
        clip.key.push_str(&format!(" switch {switch}"));
        Some(clip)
    }

    /// A ranged weapon's point (raise to aim) or unpoint: a short blend
    /// between the armed stand's first pose and the attack's first pose.
    fn point(&self, set: &ArmedSet, attack_slot: &str, raise: bool) -> Option<Clip> {
        let (stand, stand_hash) = self.sources.get(&format!("{}.stand", set.family))?;
        let (attack, attack_hash) = self.sources.get(attack_slot)?;
        let from = nif::local_pose(self.skeleton(), &stand, 0.0);
        let to = nif::local_pose(self.skeleton(), &attack, 0.0);
        let times = once_times(POINT_SECONDS, ACTION_FPS);
        let last = (times.len() - 1) as f32;
        let samples = (0..times.len())
            .map(|k| {
                let w = k as f32 / last;
                let w = if raise { w } else { 1.0 - w };
                // Ease in and out.
                let w = w * w * (3.0 - 2.0 * w);
                Sample {
                    pose: nif::compose(self.skeleton(), &nif::blend(&from, &to, w)),
                    weapon: true,
                }
            })
            .collect();
        Some(Clip {
            anim: if raise { Anim::Point } else { Anim::Unpoint },
            weapon: set.code,
            fps: ACTION_FPS,
            motion: ClipMotion::InPlace,
            samples,
            contact: Some(times.len() - 1),
            clamp_ground: false,
            source: format!(
                "blend {}.stand to {attack_slot}{}",
                set.family,
                if raise { "" } else { ", reversed" }
            ),
            key: format!("point {stand_hash} {attack_hash} {raise} {POINT_SECONDS}"),
            weapon_mesh: None,
            cycle: [0.0; 2],
        })
    }

    /// Automatic fire: the family's fire loop repeated for
    /// [`BURST_SECONDS`]; the single attack when there is no loop.
    fn burst(&self, set: &ArmedSet, anim: Anim) -> Option<Clip> {
        let slot = format!("{}.loop", set.family);
        let Some((animation, hash)) = self.sources.get(&slot) else {
            let mut clip =
                self.once(anim, set.code, &format!("{}.attack", set.family), Some(0.2))?;
            clip.source.push_str(" for want of a fire loop");
            return Some(clip);
        };
        let times = once_times(BURST_SECONDS, ACTION_FPS);
        let samples = times
            .iter()
            .map(|&t| {
                // Wrap by hand: the loop's own flag may say "clamp".
                let d = animation.duration.max(1e-3);
                let local = t - (t / d).floor() * d;
                Sample {
                    pose: nif::pose(self.skeleton(), &animation, local),
                    weapon: true,
                }
            })
            .collect();
        Some(Clip {
            anim,
            weapon: set.code,
            fps: ACTION_FPS,
            motion: ClipMotion::InPlace,
            samples,
            contact: Some(1),
            clamp_ground: false,
            source: format!("{} repeated", source_name(&slot, &animation)),
            key: format!("burst {hash} {BURST_SECONDS}"),
            weapon_mesh: None,
            cycle: [0.0; 2],
        })
    }

    /// A fall (`ba`/`bb`) from the death sampler, or the game's
    /// knock-down when the sampler is not asked or fails; the body ends
    /// lying away from its feet hex (baked).
    fn fall(&self, fall: Fall) -> Option<Clip> {
        let anim = match fall {
            Fall::Back => Anim::FallBack,
            Fall::Front => Anim::FallFront,
        };
        if self.options.deaths == DeathSource::Ragdoll {
            let start = nif::pose(self.skeleton(), &self.body.stand, 0.0);
            let seed = self.seed ^ (anim.code() as u64);
            match self.options.sampler.fall(
                self.skeleton(),
                &start,
                fall,
                seed,
                f32::from(ACTION_FPS),
            ) {
                Ok(poses) if poses.len() >= 2 => {
                    return Some(Clip {
                        anim,
                        weapon: WeaponCode::Unarmed,
                        fps: ACTION_FPS,
                        motion: ClipMotion::Baked,
                        samples: poses
                            .into_iter()
                            .map(|pose| Sample {
                                pose,
                                weapon: false,
                            })
                            .collect(),
                        contact: None,
                        clamp_ground: true,
                        source: format!("{} {fall:?}", self.options.sampler.name()),
                        key: format!(
                            "ragdoll {} {fall:?} seed {seed}",
                            self.options.sampler.name()
                        ),
                        weapon_mesh: None,
                        cycle: [0.0; 2],
                    });
                }
                Ok(_) => self
                    .notes
                    .borrow_mut()
                    .push(format!("{fall:?}: the sampler gave no fall")),
                Err(e) => self.notes.borrow_mut().push(format!("{fall:?}: {e}")),
            }
        }
        let slot = match fall {
            Fall::Back => "fall_back",
            Fall::Front => "fall_front",
        };
        let (animation, hash) = self.sources.get(slot)?;
        let times = once_times(animation.duration, ACTION_FPS);
        let samples = times
            .iter()
            .map(|&t| Sample {
                pose: baked(nif::pose(self.skeleton(), &animation, t)),
                weapon: false,
            })
            .collect();
        Some(Clip {
            anim,
            weapon: WeaponCode::Unarmed,
            fps: ACTION_FPS,
            motion: ClipMotion::Baked,
            samples,
            contact: None,
            clamp_ground: true,
            source: source_name(slot, &animation),
            key: format!("fall {hash}"),
            weapon_mesh: None,
            cycle: [0.0; 2],
        })
    }
}

/// A get-up from a fall: its poses reversed, from where it lies back to
/// where it stood, thinned to at most [`GET_UP_FRAMES`]; the still at
/// rest that ends a fall is mostly dropped first.
fn get_up(fall: &Clip) -> Clip {
    let anim = match fall.anim {
        Anim::FallFront => Anim::GetUpFront,
        _ => Anim::GetUpBack,
    };
    // Drop the trailing frames the body no longer moves in, but one.
    let moved = |a: &nif::Pose, b: &nif::Pose| {
        a.world
            .iter()
            .zip(&b.world)
            .any(|(x, y)| (0..3).any(|r| (x[r][3] - y[r][3]).abs() > 0.5))
    };
    let mut end = fall.samples.len();
    while end > 2 && !moved(&fall.samples[end - 2].pose, &fall.samples[end - 1].pose) {
        end -= 1;
    }
    let kept: Vec<&Sample> = fall.samples[..end].iter().rev().collect();
    let count = kept.len().clamp(2, GET_UP_FRAMES);
    let samples = (0..count)
        .map(|k| {
            let at = (k * (kept.len() - 1)) / (count - 1);
            kept[at].clone()
        })
        .collect();
    Clip {
        anim,
        weapon: WeaponCode::Unarmed,
        fps: fall.fps,
        motion: ClipMotion::Baked,
        samples,
        contact: None,
        clamp_ground: true,
        source: format!(
            "{} reversed",
            fall.anim
                .suffix_str(WeaponCode::Unarmed)
                .unwrap_or_default()
        ),
        key: format!("reversed {} {GET_UP_FRAMES}", fall.key),
        weapon_mesh: None,
        cycle: [0.0; 2],
    }
}

/// The corpse still of a fall: its last pose.
fn corpse(fall: &Clip) -> Clip {
    let anim = match fall.anim {
        Anim::FallFront => Anim::DeadFallFront,
        _ => Anim::DeadFallBack,
    };
    Clip {
        anim,
        weapon: WeaponCode::Unarmed,
        fps: fall.fps,
        motion: ClipMotion::Baked,
        samples: fall.samples.last().cloned().into_iter().collect(),
        contact: None,
        clamp_ground: true,
        source: format!(
            "last frame of {}",
            fall.anim
                .suffix_str(WeaponCode::Unarmed)
                .unwrap_or_default()
        ),
        key: format!("corpse {}", fall.key),
        weapon_mesh: None,
        cycle: [0.0; 2],
    }
}

/// Rebuilds an [`emit::FrmSpec`] from FRM bytes (a cached animation).
fn spec_from_frm(bytes: &[u8]) -> Result<emit::FrmSpec, String> {
    let archive = FrmArchive::parse(bytes).map_err(|e| e.to_string())?;
    let count = archive.frame_count();
    let mut directions = Vec::with_capacity(6);
    for direction in 0..6 {
        let mut frames = Vec::with_capacity(count);
        for k in 0..count {
            let frame = archive
                .frame(direction, k)
                .ok_or_else(|| format!("direction {direction} lacks frame {k}"))?;
            frames.push(emit::FrmFrameSpec {
                width: frame.width,
                height: frame.height,
                displacement: frame.displacement,
                pixels: frame.pixels().to_vec(),
            });
        }
        directions.push(emit::FrmDirection {
            shift: archive.shifts[direction],
            frames,
        });
    }
    Ok(emit::FrmSpec {
        fps: u16::try_from(archive.fps).unwrap_or(0),
        action_frame: u16::try_from(archive.action_frame).unwrap_or(0),
        directions,
    })
}

/// Walk measurements of one direction: hexes a cycle covers and how far
/// the planted foot slides on screen per cycle.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StrideMeasure {
    /// Hexes a cycle covers with [`Stride::Matched`].
    pub hexes_per_cycle: f32,
    /// Pixels the planted foot moves per cycle with [`Stride::Exact`].
    pub slide_exact: f32,
    /// The same with [`Stride::Matched`].
    pub slide_matched: f32,
    /// The same with [`Stride::WholeHexes`].
    pub slide_whole: f32,
}

/// Screen travel per frame of one direction scaled to whole hexes for
/// [`Stride::WholeHexes`] (the widening of [`Stride::Matched`] is applied
/// before), and the cycle's hexes along the step before scaling.
fn stride_travel(direction: usize, travel: &[[f32; 2]], stride: Stride) -> (Vec<[f32; 2]>, f32) {
    let step = emit::HEX_STEPS[direction];
    let length2 = (step[0] * step[0] + step[1] * step[1]) as f32;
    let hexes: f32 = travel
        .iter()
        .map(|t| (t[0] * step[0] as f32 + t[1] * step[1] as f32) / length2)
        .sum();
    let scaled = match stride {
        Stride::Exact | Stride::Matched => travel.to_vec(),
        Stride::WholeHexes if hexes > 0.0 => {
            let factor = hexes.round().max(1.0) / hexes;
            travel
                .iter()
                .map(|t| [t[0] * factor, t[1] * factor])
                .collect()
        }
        Stride::WholeHexes => travel.to_vec(),
    };
    (scaled, hexes)
}

/// Both feet of one frame: screen position (in place) and height.
type Feet = [([f32; 2], f32); 2];

/// The slide of the planted foot over one cycle: `feet` the screen
/// positions (in place) and heights of the two feet per frame, `offsets`
/// the frame offsets the engine adds.
fn foot_slide(feet: &[Feet], offsets: &[[i16; 2]]) -> f32 {
    if feet.len() < 2 || offsets.len() != feet.len() {
        return 0.0;
    }
    let lowest = feet
        .iter()
        .flat_map(|f| f.iter().map(|(_, z)| *z))
        .fold(f32::INFINITY, f32::min);
    let mut accumulated = [0.0f32; 2];
    let mut positions = Vec::with_capacity(feet.len());
    for (frame, offset) in feet.iter().zip(offsets) {
        accumulated[0] += f32::from(offset[0]);
        accumulated[1] += f32::from(offset[1]);
        positions.push(frame.map(|(p, z)| ([p[0] + accumulated[0], p[1] + accumulated[1]], z)));
    }
    let mut slide = 0.0;
    for pair in positions.windows(2) {
        // The planted foot: the lower one in both frames, near the ground.
        for foot in 0..2 {
            let other = 1 - foot;
            let (a, za) = pair[0][foot];
            let (b, zb) = pair[1][foot];
            if za <= pair[0][other].1 && zb <= pair[1][other].1 && za.max(zb) < lowest + 3.0 {
                slide += ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            }
        }
    }
    slide
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

    /// The key of a set above the minimal level: [`Shared::key`]'s inputs,
    /// every further animation and the armed sets.
    fn animated_key(&self, doc: &ActorDoc, armed: &[ArmedSet]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.key(doc).as_bytes());
        if let Some(animations) = &doc.animations {
            for (slot, hash) in &animations.more {
                hasher.update(format!("{slot}={hash};").as_bytes());
            }
        }
        for set in armed {
            hasher.update(
                format!(
                    "armed {:?} {:?} {} {}|",
                    set.code,
                    set.modes,
                    set.family,
                    set.mesh.as_deref().unwrap_or("-")
                )
                .as_bytes(),
            );
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

    /// Renders and writes one sprite set at the minimal level. `Ok(true)`
    /// when it was rendered, `Ok(false)` when the cache already had it.
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
            schema: 0,
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
            level: None,
            real: BTreeMap::new(),
            placeholders: Vec::new(),
            weapons: BTreeMap::new(),
            bytes: 0,
        };
        let text = serde_json::to_string_pretty(&meta).expect("plain data");
        std::fs::write(&meta_path, text + "\n")
            .map_err(|e| format!("{}: {e}", meta_path.display()))?;
        Ok(true)
    }
}

/// FRM bytes of a spec, checked to parse back.
fn encode(spec: &emit::FrmSpec, what: &str) -> Result<Vec<u8>, String> {
    let bytes =
        emit::FormatWriters::frm(&emit::LocalWriters, spec).map_err(|e| format!("{what}: {e}"))?;
    // Never leave a file the engine's own parser would refuse.
    FrmArchive::parse(&bytes).map_err(|e| format!("{what} does not parse back: {e}"))?;
    Ok(bytes)
}

/// Renders clips for one set; holds what all its clips share.
struct SetRenderer<'r, 'a> {
    shared: &'r Shared<'a>,
    body: &'r Body,
    look: FigureLook,
    stride: Stride,
    renderer: &'r mut dyn foundry_render::Renderer,
    textures: &'r MirTextures<'a>,
    /// Weapon meshes by hash.
    weapons: BTreeMap<String, nif::StaticMesh>,
    weapon_bone: Option<u32>,
    feet: [Option<u32>; 2],
    missing: BTreeSet<String>,
    /// Everything of the set's cache key but the clip.
    key_prefix: String,
    /// Walk measurements, by suffix.
    strides: BTreeMap<String, [StrideMeasure; 6]>,
}

impl SetRenderer<'_, '_> {
    /// The cache path of a clip.
    fn cache_path(&self, clip: &Clip) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(self.key_prefix.as_bytes());
        hasher.update(
            format!(
                "|{} {:?} {} {:?} {} {} {}|",
                clip.anim.code(),
                clip.weapon,
                clip.fps,
                clip.motion,
                clip.clamp_ground,
                clip.weapon_mesh.as_deref().unwrap_or("-"),
                clip.key
            )
            .as_bytes(),
        );
        let marks: Vec<u8> = clip.samples.iter().map(|s| u8::from(s.weapon)).collect();
        hasher.update(&marks);
        hasher.update(format!("{:?}", clip.contact).as_bytes());
        self.shared
            .dir
            .join("anims")
            .join(format!("{}.frm", hex(&hasher.finalize())))
    }

    /// The posed meshes of one sample, the weapon included when it shows.
    fn posed(&self, clip: &Clip, sample: &Sample) -> Vec<(nif::StaticMesh, bool)> {
        let mut meshes: Vec<(nif::StaticMesh, bool)> = self
            .body
            .posed(&sample.pose)
            .into_iter()
            .zip(&self.body.parts)
            .map(|(mesh, part)| (mesh, part.tinted))
            .collect();
        if sample.weapon
            && let (Some(hash), Some(bone)) = (&clip.weapon_mesh, self.weapon_bone)
            && let (Some(mesh), Some(matrix)) =
                (self.weapons.get(hash), sample.pose.world.get(bone as usize))
        {
            let mut mesh = mesh.clone();
            transform_mesh(&mut mesh, matrix);
            meshes.push((mesh, false));
        }
        if clip.clamp_ground {
            let lowest = meshes
                .iter()
                .flat_map(|(m, _)| m.primitives.iter().flat_map(|p| p.positions.iter()))
                .map(|p| p[2])
                .fold(f32::INFINITY, f32::min);
            if lowest.is_finite() && lowest < 0.0 {
                let up = [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, -lowest],
                ];
                for (mesh, _) in &mut meshes {
                    transform_mesh(mesh, &up);
                }
            }
        }
        meshes
    }

    /// The screen position (in place) and height of each foot.
    fn feet(&self, pose: &nif::Pose, direction: usize) -> Feet {
        let figure = figure_transform_with(self.shared.yaws[direction], 1.0, self.look.girth);
        self.feet.map(|bone| {
            let Some(m) = bone.and_then(|b| pose.world.get(b as usize)) else {
                return ([0.0; 2], 0.0);
            };
            let p = [m[0][3], m[1][3], m[2][3]];
            let w: [f32; 3] = std::array::from_fn(|r| {
                figure[r][0] * p[0] + figure[r][1] * p[1] + figure[r][2] * p[2] + figure[r][3]
            });
            let screen = self
                .shared
                .projection
                .screen(self.shared.projection.tile_from_source([0.0; 3], w));
            (screen, p[2])
        })
    }

    /// Renders a clip to an FRM spec, or takes it from the cache.
    fn render(&mut self, clip: &Clip) -> Result<(emit::FrmSpec, bool), String> {
        let path = self.cache_path(clip);
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(spec) = spec_from_frm(&bytes)
        {
            return Ok((spec, false));
        }
        let tinted = Tinted {
            inner: self.textures,
            tint: self.body.hair,
        };
        // Travel since the previous frame, root axes; a cycle's frame 0
        // follows its last frame.
        let count = clip.samples.len();
        let steps: Vec<[f32; 2]> = match clip.motion {
            ClipMotion::Cycle => {
                let last = clip.samples[count - 1].pose.travel;
                // The cycle's total, as the walk's own end pose gives it.
                let total = clip.cycle;
                let mut previous = [last[0] - total[0], last[1] - total[1]];
                clip.samples
                    .iter()
                    .map(|s| {
                        let step = [
                            s.pose.travel[0] - previous[0],
                            s.pose.travel[1] - previous[1],
                        ];
                        previous = s.pose.travel;
                        step
                    })
                    .collect()
            }
            _ => vec![[0.0; 2]; count],
        };
        let mut directions: [Vec<emit::RenderedFrame>; 6] = Default::default();
        let mut feet: [Vec<Feet>; 6] = Default::default();
        for (sample, step) in clip.samples.iter().zip(&steps) {
            let posed = self.posed(clip, sample);
            let meshes: Vec<nif::StaticMesh> = posed.iter().map(|(m, _)| m.clone()).collect();
            for (direction, frames) in directions.iter_mut().enumerate() {
                let yaw = self.shared.yaws[direction];
                let turn = mir::Transform {
                    position: [0.0; 3],
                    rotation: [0.0, 0.0, yaw],
                    scale: 1.0,
                }
                .matrix();
                let figure = figure_transform_with(yaw, 1.0, self.look.girth);
                let scene = Scene {
                    anchor: [0.0; 3],
                    instances: meshes
                        .iter()
                        .zip(&posed)
                        .map(|(mesh, (_, is_tinted))| {
                            Instance::new(
                                mesh,
                                figure,
                                if *is_tinted {
                                    &tinted as &dyn foundry_render::TextureSource
                                } else {
                                    self.textures
                                },
                            )
                        })
                        .collect(),
                };
                let mut output = self
                    .renderer
                    .render(&scene, Window::Fit)
                    .map_err(|e| format!("render: {e}"))?;
                lift_with(&mut output.rgba, self.look.lift_gamma);
                if !self.look.tone.is_identity() {
                    self.look.tone.apply(&mut output.rgba);
                }
                self.missing.extend(output.missing_textures.iter().cloned());
                let mut pixels = quantize::quantize(
                    &output.rgba,
                    output.width,
                    output.height,
                    self.shared.map,
                    Dither::None,
                )
                .map_err(|e| format!("quantize: {e}"))?;
                let (mut width, mut height, mut origin) =
                    (output.width, output.height, output.origin);
                if let Some(rim) = &self.look.outline {
                    let rimmed = quantize::rim(&pixels, width, height, self.shared.map, rim)
                        .map_err(|e| format!("outline: {e}"))?;
                    pixels = rimmed.pixels;
                    width = rimmed.width;
                    height = rimmed.height;
                    origin = [origin[0] + rimmed.offset[0], origin[1] + rimmed.offset[1]];
                }
                // The step, turned with the figure; widened with it
                // unless the stride is the source's own.
                let widen = if self.stride == Stride::Exact {
                    1.0
                } else {
                    self.look.girth
                };
                let ground = [
                    (turn[0][0] * step[0] + turn[0][1] * step[1]) * widen,
                    (turn[1][0] * step[0] + turn[1][1] * step[1]) * widen,
                    0.0,
                ];
                let travel = self
                    .shared
                    .projection
                    .screen(self.shared.projection.tile_from_source([0.0; 3], ground));
                frames.push(emit::RenderedFrame {
                    origin,
                    width,
                    height,
                    pixels,
                    travel,
                });
                if clip.motion == ClipMotion::Cycle {
                    feet[direction].push(self.feet(&sample.pose, direction));
                }
            }
        }
        if clip.motion == ClipMotion::Cycle {
            let mut measures = [StrideMeasure::default(); 6];
            // The frames carry the travel of the chosen stride; measure all
            // three from the unwidened one.
            let widen = if self.stride == Stride::Exact {
                1.0
            } else {
                self.look.girth
            };
            for (direction, frames) in directions.iter_mut().enumerate() {
                let travel: Vec<[f32; 2]> = frames.iter().map(|f| f.travel).collect();
                let exact: Vec<[f32; 2]> = travel
                    .iter()
                    .map(|t| [t[0] / widen, t[1] / widen])
                    .collect();
                let matched: Vec<[f32; 2]> = exact
                    .iter()
                    .map(|t| [t[0] * self.look.girth, t[1] * self.look.girth])
                    .collect();
                let (whole, hexes) = stride_travel(direction, &matched, Stride::WholeHexes);
                let offsets = |t: &[[f32; 2]]| emit::critter::travel_offsets(direction, t);
                let slide =
                    |t: &[[f32; 2]]| offsets(t).map_or(0.0, |o| foot_slide(&feet[direction], &o));
                measures[direction] = StrideMeasure {
                    hexes_per_cycle: hexes,
                    slide_exact: slide(&exact),
                    slide_matched: slide(&matched),
                    slide_whole: slide(&whole),
                };
                let (chosen, _) = stride_travel(direction, &travel, self.stride);
                for (frame, t) in frames.iter_mut().zip(chosen) {
                    frame.travel = t;
                }
            }
            if let Ok(suffix) = clip.anim.suffix_str(clip.weapon) {
                self.strides.insert(suffix, measures);
            }
        }
        let frames = emit::CritterFrames {
            fps: clip.fps,
            directions,
        };
        let action_frame = match clip.contact {
            Some(contact) => emit::critter::action_frame_for_contact(contact.min(count - 1), count)
                .map_err(|e| e.to_string())?,
            None => 0,
        };
        let motion = match clip.motion {
            ClipMotion::Cycle | ClipMotion::InPlace => emit::Motion::Cycle,
            ClipMotion::Baked => emit::Motion::Baked,
        };
        let spec = emit::critter_frm_with(
            &frames,
            &emit::FrmOptions {
                action_frame,
                motion,
            },
        )
        .map_err(|e| e.to_string())?;
        let bytes = encode(&spec, &clip.source)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((spec, true))
    }
}

impl Clip {
    /// Source units a cycle travels.
    fn cycle_units(&self) -> f32 {
        (self.cycle[0] * self.cycle[0] + self.cycle[1] * self.cycle[1]).sqrt()
    }
}

impl Shared<'_> {
    /// The armed sets a document gets at `level`.
    fn armed_sets(doc: &ActorDoc, options: &CritterOptions) -> Vec<ArmedSet> {
        let mut out: Vec<ArmedSet> = Vec::new();
        let mut push = |set: ArmedSet| {
            if !out.iter().any(|s| s.code == set.code) {
                out.push(set);
            }
        };
        for weapon in &doc.weapons {
            if !weapon.carried && options.level != AnimationLevel::Full {
                continue;
            }
            let chosen = weapon
                .form
                .as_ref()
                .and_then(|f| options.weapon_codes.get(&f.to_string()).copied())
                .or_else(|| family_weapon(&weapon.family));
            let Some((code, modes)) = chosen else {
                continue;
            };
            if code == WeaponCode::Unarmed {
                continue;
            }
            push(ArmedSet {
                code,
                modes,
                family: weapon.family.clone(),
                mesh: weapon.mesh.clone(),
                path: weapon.path.clone(),
            });
        }
        out.sort_by_key(|s| s.code);
        out
    }

    /// Renders and writes one sprite set above the minimal level.
    fn build_animated(
        &self,
        doc: &ActorDoc,
        key: &str,
        options: &CritterOptions,
        renderer: &mut dyn foundry_render::Renderer,
        textures: &MirTextures<'_>,
    ) -> Result<Option<String>, String> {
        let started = Instant::now();
        let dir = self.dir.join(key);
        let meta_path = dir.join("set.json");
        if meta_path.is_file() {
            return Ok(None);
        }
        let body = self.body(doc)?;
        let animations = doc.animations.as_ref().ok_or("no animations")?;
        let armed = Shared::armed_sets(doc, options);
        // The falls' seed: the body's content only (appearance, skeleton,
        // stand), so the same person falls the same way whatever the look,
        // level or weapons.
        let mut seed_bytes = [0u8; 8];
        let body_key = format!(
            "{} {} {}",
            doc.appearance,
            doc.skeleton.as_deref().unwrap_or(""),
            animations.stand
        );
        seed_bytes.copy_from_slice(&Sha256::digest(body_key.as_bytes())[..8]);
        let maker = ClipMaker {
            body: &body,
            sources: Sources {
                mir: self.mir,
                hashes: &animations.more,
                cache: RefCell::new(BTreeMap::new()),
            },
            options,
            seed: u64::from_le_bytes(seed_bytes),
            notes: RefCell::new(Vec::new()),
        };

        // The clips, in a fixed order.
        let mut clips: Vec<Clip> = vec![
            maker.looping(
                Anim::Stand,
                WeaponCode::Unarmed,
                &body.stand,
                &animations.stand,
                STAND_FPS,
                false,
                "stand".to_owned(),
            ),
            maker.looping(
                Anim::Walk,
                WeaponCode::Unarmed,
                &body.walk,
                &animations.walk,
                WALK_FPS,
                true,
                "walk".to_owned(),
            ),
        ];
        if let Some((run, hash)) = maker.sources.get("run") {
            clips.push(maker.looping(
                Anim::Run,
                WeaponCode::Unarmed,
                &run,
                &hash,
                WALK_FPS,
                true,
                source_name("run", &run),
            ));
        }
        let unarmed = WeaponCode::Unarmed;
        for (anim, slot, contact) in [
            (Anim::Dodge, "dodge", Some(0.5)),
            (Anim::HitFront, "hit_front", None),
            (Anim::Punch, "punch", Some(0.5)),
            (Anim::Kick, "kick", Some(0.5)),
            (Anim::GroundReach, "reach_ground", Some(0.5)),
            (Anim::MiddleReach, "reach_middle", Some(0.5)),
        ] {
            clips.extend(maker.once(anim, unarmed, slot, contact));
        }
        for fall in [Fall::Back, Fall::Front] {
            if let Some(clip) = maker.fall(fall) {
                let up = get_up(&clip);
                let dead = corpse(&clip);
                clips.push(clip);
                clips.push(up);
                clips.push(dead);
            }
        }
        let mut weapon_meshes = BTreeMap::new();
        for set in &armed {
            let start = clips.len();
            clips.extend(maker.armed_stand(set));
            clips.extend(maker.armed_walk(set, &animations.walk));
            clips.extend(maker.armed_dodge(set));
            clips.extend(maker.equip(set, true));
            clips.extend(maker.equip(set, false));
            let attack = format!("{}.attack", set.family);
            let mut ranged = false;
            for mode in set.modes {
                let Some(anim) = mode.animation() else {
                    continue;
                };
                match mode {
                    AttackMode::Burst | AttackMode::Continuous => {
                        clips.extend(maker.burst(set, anim));
                    }
                    AttackMode::Throw | AttackMode::Punch | AttackMode::Kick => {}
                    _ => clips.extend(maker.once(anim, set.code, &attack, Some(0.3))),
                }
                ranged |= mode.is_ranged_fire();
            }
            if ranged {
                clips.extend(maker.point(set, &attack, true));
                clips.extend(maker.point(set, &attack, false));
            }
            for clip in &mut clips[start..] {
                clip.weapon_mesh.clone_from(&set.mesh);
            }
            if let Some(hash) = &set.mesh {
                let mesh = self
                    .mir
                    .mesh(hash)
                    .map_err(|e| format!("weapon {}: {e}", set.path))?;
                weapon_meshes.insert(hash.clone(), mesh);
            }
        }

        let weapon_bone = WEAPON_BONES.iter().find_map(|b| body.skeleton.find(b));
        if weapon_bone.is_none() && !armed.is_empty() {
            maker
                .notes
                .borrow_mut()
                .push("the skeleton has no weapon bone: weapons are not drawn".to_owned());
        }
        let feet = ["Bip01 L Foot", "Bip01 R Foot"].map(|b| body.skeleton.find(b));
        let mut render = SetRenderer {
            shared: self,
            body: &body,
            look: options.look,
            stride: options.stride,
            renderer,
            textures,
            weapons: weapon_meshes,
            weapon_bone,
            feet,
            missing: BTreeSet::new(),
            key_prefix: format!(
                "{} {} {}",
                hex(&self.key_prefix),
                self.key(doc),
                options.look.key()
            ) + &format!(" stride {:?}", options.stride),
            strides: BTreeMap::new(),
        };

        let mut set = AnimationSet::new();
        let mut real: BTreeMap<String, AnimationNote> = BTreeMap::new();
        let mut rendered = 0usize;
        let mut stand_frames = 0;
        let mut walk_frames = 0;
        let mut walk_units = 0.0;
        for clip in &clips {
            if set.contains(clip.anim, clip.weapon) {
                continue;
            }
            let (spec, fresh) = render.render(clip)?;
            rendered += usize::from(fresh);
            let suffix = clip
                .anim
                .suffix_str(clip.weapon)
                .map_err(|e| e.to_string())?;
            let frames = spec.directions[0].frames.len();
            if clip.anim == Anim::Stand && clip.weapon == unarmed {
                stand_frames = frames;
            }
            if clip.anim == Anim::Walk && clip.weapon == unarmed {
                walk_frames = frames;
                walk_units = clip.cycle_units();
            }
            real.insert(
                suffix,
                AnimationNote {
                    source: clip.source.clone(),
                    frames,
                    fps: spec.fps,
                    action_frame: spec.action_frame,
                },
            );
            set.insert(clip.anim, clip.weapon, spec)
                .map_err(|e| e.to_string())?;
        }
        let weapons: Vec<(WeaponCode, [AttackMode; 2])> =
            armed.iter().map(|s| (s.code, s.modes)).collect();
        let requirements = emit::animation::combat_requirements(&weapons);
        let filled = set
            .fill_placeholders(&requirements, Need::Required)
            .map_err(|e| e.to_string())?;
        let placeholders: Vec<String> = filled.iter().filter_map(|r| r.suffix().ok()).collect();
        let missing_required = set.missing(&requirements, Need::Required);
        if !missing_required.is_empty() {
            return Err(format!(
                "the set still lacks {} required animations",
                missing_required.len()
            ));
        }

        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut bytes_total = 0u64;
        let mut suffixes = Vec::new();
        for (suffix, spec) in set.by_suffix().map_err(|e| e.to_string())? {
            let bytes = encode(spec, &suffix)?;
            bytes_total += bytes.len() as u64;
            let path = dir.join(format!("{suffix}.frm"));
            std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            suffixes.push(suffix);
        }
        let meta = CritterSetMeta {
            schema: SET_SCHEMA,
            appearance: doc.appearance.clone(),
            label: doc
                .editor_id
                .clone()
                .unwrap_or_else(|| doc.form.to_string()),
            stand_frames,
            walk_frames,
            walk_cycle_units: walk_units,
            animations: suffixes,
            missing_textures: std::mem::take(&mut render.missing).into_iter().collect(),
            level: Some(options.level),
            real: real.clone(),
            placeholders: placeholders.clone(),
            weapons: armed
                .iter()
                .map(|s| {
                    (
                        s.code
                            .letter()
                            .map_or_else(|| "-".to_owned(), |l| char::from(l).to_string()),
                        s.path.clone(),
                    )
                })
                .collect(),
            bytes: bytes_total,
        };
        let text = serde_json::to_string_pretty(&meta).expect("plain data");
        std::fs::write(&meta_path, text + "\n")
            .map_err(|e| format!("{}: {e}", meta_path.display()))?;
        let frames: usize = real.values().map(|n| n.frames).sum();
        let mut line = format!(
            "{} ({}): {} real [{}], {} placeholders [{}], {} frames x 6 ({} rendered now), {} bytes, {:.1} s",
            meta.label,
            &key[..12],
            real.len(),
            real.keys().cloned().collect::<Vec<_>>().join(" "),
            placeholders.len(),
            placeholders.join(" "),
            frames,
            rendered,
            bytes_total,
            started.elapsed().as_secs_f32()
        );
        for (suffix, measures) in &render.strides {
            let hexes: f32 = measures.iter().map(|m| m.hexes_per_cycle).sum::<f32>() / 6.0;
            let exact: f32 = measures.iter().map(|m| m.slide_exact).sum::<f32>() / 6.0;
            let matched: f32 = measures.iter().map(|m| m.slide_matched).sum::<f32>() / 6.0;
            let whole: f32 = measures.iter().map(|m| m.slide_whole).sum::<f32>() / 6.0;
            line.push_str(&format!(
                "; {suffix}: {hexes:.2} hexes a cycle, foot slide per cycle {exact:.1} px exact, \
                 {matched:.1} px matched, {whole:.1} px whole-hex"
            ));
        }
        for note in maker.notes.borrow().iter() {
            line.push_str(&format!("; note: {note}"));
        }
        Ok(Some(line))
    }
}

/// Renders a sprite set for every distinct human appearance in the slice
/// to `<out>/critters/`, with the default options.
pub fn critters(inputs: &Inputs) -> Result<CrittersSummary, ConvertError> {
    critters_with(inputs, &CritterOptions::default())
}

/// [`critters`] with options.
pub fn critters_with(
    inputs: &Inputs,
    options: &CritterOptions,
) -> Result<CrittersSummary, ConvertError> {
    let stage = |e: &dyn std::fmt::Display| ConvertError::stage("critters", e);
    let mir = Mir::open(&inputs.mir_dir()).map_err(|e| stage(&e))?;
    let palette_bytes = inputs.f2_file("color.pal")?;
    let palette = inputs.palette()?;
    let map = PaletteMap::try_new(&palette).map_err(|e| stage(&e))?;
    let dir = inputs.critters_dir();
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let yaws = direction_yaws(inputs)?;
    let minimal = options.level == AnimationLevel::Minimal;
    if minimal && !options.look.is_default() {
        return Err(stage(
            &"the minimal level draws figures as the first wave did; tone and outline need another level",
        ));
    }

    // The render path is part of the key, as for sprites.
    let backend = match inputs.backend {
        Backend::Auto => foundry_render::renderer(Backend::Auto, &inputs.calibration)
            .map_err(|e| stage(&e))?
            .backend(),
        other => other,
    };
    let mut prefix = Sha256::new();
    prefix.update(if minimal {
        STAGE_VERSION.as_bytes()
    } else {
        ANIMATED_STAGE_VERSION.as_bytes()
    });
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
    if !minimal {
        prefix.update(
            format!(
                "level {} look {} deaths {:?} {} stride {:?} action {ACTION_FPS}",
                options.level.name(),
                options.look.key(),
                options.deaths,
                options.sampler.name(),
                options.stride
            )
            .as_bytes(),
        );
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
    let selected = |doc: &ActorDoc| {
        options.only.is_empty()
            || doc.editor_id.as_deref().is_some_and(|id| {
                options
                    .only
                    .iter()
                    .any(|wanted| wanted.eq_ignore_ascii_case(id))
            })
    };
    let mut keys: Vec<Result<String, String>> = Vec::with_capacity(docs.len());
    let mut jobs: BTreeMap<String, usize> = BTreeMap::new();
    for (index, doc) in docs.iter().enumerate() {
        let key = Shared::renderable(doc)
            .and_then(|()| {
                if selected(doc) {
                    Ok(())
                } else {
                    Err("not selected for this run".to_owned())
                }
            })
            .map(|()| {
                if minimal {
                    shared.key(doc)
                } else {
                    shared.animated_key(doc, &Shared::armed_sets(doc, options))
                }
            });
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
    type Outcome = Result<Option<String>, String>;
    let run = |slice: &[(String, usize)]| -> Result<Vec<Outcome>, String> {
        let mut renderer =
            foundry_render::renderer(backend, &inputs.calibration).map_err(|e| e.to_string())?;
        let textures = MirTextures {
            mir: &mir,
            cache: RefCell::new(BTreeMap::new()),
        };
        Ok(slice
            .iter()
            .map(|(key, index)| {
                let doc = &docs_ref[*index];
                if minimal {
                    shared_ref
                        .build(doc, key, renderer.as_mut(), &textures)
                        .map(|fresh| fresh.then(String::new))
                } else {
                    shared_ref.build_animated(doc, key, options, renderer.as_mut(), &textures)
                }
            })
            .collect())
    };
    let results: Vec<Outcome> = std::thread::scope(|scope| {
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
            Ok(Some(line)) => {
                summary.rendered += 1;
                if !line.is_empty() {
                    summary.lines.push(line.clone());
                }
            }
            Ok(None) => summary.reused += 1,
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
    if !minimal {
        write_report(inputs, &index)?;
    }
    Ok(summary)
}

/// `report.md`: per set, real and placeholder animations, from the sets'
/// own `set.json`, so it holds for reused sets too.
fn write_report(inputs: &Inputs, index: &[CritterEntry]) -> Result<(), ConvertError> {
    let mut seen = BTreeSet::new();
    let mut text = String::from(
        "# Critter sets\n\nOne line per sprite set: the animations rendered from the source \
         (with where each came from) and those that are a still of the stand.\n\n",
    );
    for entry in index {
        let Some(key) = &entry.set else { continue };
        if !seen.insert(key.clone()) {
            continue;
        }
        let meta = read_meta(inputs, key)?;
        text.push_str(&format!(
            "- **{}** `{}`: {} real, {} placeholders [{}]; {} bytes\n",
            meta.label,
            &key[..12.min(key.len())],
            meta.real.len(),
            meta.placeholders.len(),
            meta.placeholders.join(" "),
            meta.bytes
        ));
        for (suffix, note) in &meta.real {
            text.push_str(&format!(
                "  - `{suffix}` {} frames at {} fps, action frame {}: {}\n",
                note.frames, note.fps, note.action_frame, note.source
            ));
        }
    }
    let path = inputs.critters_dir().join("report.md");
    std::fs::write(&path, text).map_err(io(&path))
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

/// Reads `<key>/set.json`.
pub fn read_meta(inputs: &Inputs, key: &str) -> Result<CritterSetMeta, ConvertError> {
    let meta_path = inputs.critters_dir().join(key).join("set.json");
    let text = std::fs::read_to_string(&meta_path).map_err(io(&meta_path))?;
    serde_json::from_str(&text).map_err(|e| ConvertError::stage("critters", e))
}

/// The animation files of sprite set `key`, by suffix.
pub fn read_set(inputs: &Inputs, key: &str) -> Result<BTreeMap<String, Vec<u8>>, ConvertError> {
    let dir = inputs.critters_dir().join(key);
    let meta = read_meta(inputs, key)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_default_to_combat() {
        assert_eq!(AnimationLevel::default(), AnimationLevel::Combat);
        for level in [
            AnimationLevel::Minimal,
            AnimationLevel::Combat,
            AnimationLevel::Full,
        ] {
            assert_eq!(level.name().parse::<AnimationLevel>(), Ok(level));
        }
        assert!("most".parse::<AnimationLevel>().is_err());
    }

    #[test]
    fn once_times_include_both_ends() {
        let times = once_times(1.0, 10);
        assert_eq!(times.len(), 11);
        assert_eq!(times[0], 0.0);
        assert_eq!(*times.last().unwrap(), 1.0);
        assert_eq!(once_times(0.0, 10), [0.0, 0.0]);
    }

    #[test]
    fn whole_hex_stride_lands_cycles_on_hexes() {
        // East: a hex is 32 pixels. 1.4 hexes in 4 frames rounds to 1.
        let travel = vec![[11.2f32, 0.0]; 4];
        let (exact, hexes) = stride_travel(1, &travel, Stride::Exact);
        assert!((hexes - 1.4).abs() < 1e-5);
        assert_eq!(exact, travel);
        let (whole, _) = stride_travel(1, &travel, Stride::WholeHexes);
        let total: f32 = whole.iter().map(|t| t[0]).sum();
        assert!((total - 32.0).abs() < 1e-3, "{total}");
        let offsets = emit::critter::travel_offsets(1, &whole).unwrap();
        assert_eq!(offsets.iter().map(|o| i32::from(o[0])).sum::<i32>(), 32);
    }

    #[test]
    fn a_planted_foot_that_moves_with_the_offsets_does_not_slide() {
        // The foot drawn in place moves back 8 pixels a frame while the
        // offsets carry the figure forward 8: it stays put.
        let feet: Vec<[([f32; 2], f32); 2]> = (0..4)
            .map(|k| [([-8.0 * k as f32, 0.0], 0.0), ([0.0, 0.0], 10.0)])
            .collect();
        assert_eq!(foot_slide(&feet, &[[0, 0], [8, 0], [8, 0], [8, 0]]), 0.0);
        // Offsets a quarter too long: 2 pixels a frame of slide.
        let slide = foot_slide(&feet, &[[0, 0], [10, 0], [10, 0], [10, 0]]);
        assert!((slide - 6.0).abs() < 1e-4, "{slide}");
    }

    #[test]
    fn family_weapons_have_codes_and_modes() {
        assert_eq!(
            family_weapon("1hp"),
            Some((WeaponCode::Pistol, [AttackMode::Single, AttackMode::None]))
        );
        assert_eq!(
            family_weapon("2ha").map(|w| w.1[1]),
            Some(AttackMode::Burst)
        );
        assert_eq!(family_weapon("h2h"), None);
        assert_eq!(family_weapon("1gt"), None);
        for family in mir::ARMED_FAMILIES {
            assert!(family_weapon(family).is_some(), "{family}");
        }
    }
}
