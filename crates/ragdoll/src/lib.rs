//! Offline, deterministic ragdoll death sampler.
//!
//! Given a skeleton ([`nif::Skeleton`]) and its pose at the moment of
//! death ([`nif::Pose`]), [`simulate`] turns the bones into rigid bodies
//! joined by limited joints, pushes them with a kill [`Impulse`], lets the
//! body collapse onto flat ground under gravity with
//! [rapier](https://rapier.rs), and samples the fall into a
//! [`RagdollClip`]: one [`nif::Pose`] per frame, ending on a pose at rest.
//! The existing renderer skins those poses into a death animation, and the
//! last one is the corpse still. It runs at conversion time only.
//!
//! # Using it from the critter stage
//!
//! ```no_run
//! # fn main() -> Result<(), ragdoll::RagdollError> {
//! # let skeleton = ragdoll::fixture::humanoid();
//! # let stand = nif::Animation::default();
//! let spec = ragdoll::RagdollSpec::humanoid();
//! let start = nif::pose(&skeleton, &stand, 0.0); // the stand's first frame
//! let impulse = ragdoll::Impulse::preset(ragdoll::Preset::FallBackward);
//! let clip = ragdoll::simulate(&skeleton, &start, &spec, &impulse, 1, 4.0, 10.0)?;
//! for pose in &clip.poses {
//!     // skin, turn to each of the six facings, render, quantize: as for `aa`
//! }
//! let corpse = clip.corpse(); // the `ra` still
//! # let _ = corpse; Ok(()) }
//! ```
//!
//! - **Start pose.** The last frame of the hit animation (`ao`) once that
//!   is real, so the fall continues the flinch; until then the first frame
//!   of the stand (`nif::pose(skeleton, stand, 0.0)`), which matches the
//!   placeholder `ao`. The presets were tuned on a standing pose with the
//!   arms at the sides; [`fixture::stand`] makes one from any `Bip01` rest
//!   pose (the rest pose itself is an A-pose and also works).
//! - **Presets and animations** (`docs/contracts.md` 10.1):
//!   [`Preset::FallBackward`] bakes `ba` (fall back, code 20) and its last
//!   pose `ra` (corpse on its back, 48); [`Preset::FallForward`] bakes `bb`
//!   (fall front, 21) and `rb` (taking 48 and 49 as back and front, in the
//!   order of 20 and 21). [`Preset::Crumple`] and [`Preset::ThrownBack`]
//!   are variants (a quiet death, an explosion) for later death codes.
//!   [`Preset::fall_suffix`] and [`Preset::corpse_suffix`] give the
//!   suffixes. The get-ups `ch` and `cj` are not physics; playing `ba` /
//!   `bb` backwards is a stopgap.
//! - **Facing.** Pushes are in the figure's own frame, so one simulation
//!   serves all six directions: render the clip turned by each
//!   `direction_yaws` entry, exactly as the stand and walk are. A different
//!   `seed` per direction gives six different falls when wanted (the
//!   engine reads per-direction `.fr0`–`.fr5` only for falls and deaths).
//! - **Sampling.** 10 samples a second (the engine's default fps; stored
//!   fps 0 means 10). On the synthetic humanoid the presets settle in 1.0
//!   to 2.7 s, so a death is 11 to 31 frames; pass `duration` 4 s (a body
//!   still moving then is damped to rest, see [`RagdollClip::forced`]),
//!   and keep every sample or thin them. The clip ends on the first sample
//!   after the body has been at rest for [`RagdollSpec::rest_time`], so
//!   its length varies by preset and seed.
//! - **Root motion.** The poses move in root space: the body ends lying
//!   away from where its feet stood (pelvis about 63 units behind for
//!   `ba`, 75 ahead for `bb`, 100 behind when thrown). [`RagdollClip::root_travel`]
//!   (and the per-frame [`RagdollClip::pelvis_travel`]) give the pelvis
//!   offset along the figure's right and forward, in source units, for
//!   frame offsets; [`RagdollClip::in_place`] gives the same poses held in
//!   place with that offset as each pose's `travel` instead. The `travel`
//!   of `emit::critter_frm` is walk-specific (only its component along the
//!   hex step counts), so a death frame's offsets want their own path.
//! - **Cost.** 14 bodies, 13 joints (and 13 muscle-tone joints while the
//!   tone lasts), 1/60 s steps with 16 substeps: 15 to 45 ms of one core
//!   per death on the development machine, release or dev profile alike
//!   (the dev profile builds dependencies at opt-level 2). Run one death
//!   per appearance and preset (or per direction, with six seeds), not per
//!   placed actor; simulations are independent, so they parallelize across
//!   actors on threads without changing a bit.
//!
//! # Units
//!
//! The skeleton stays in its own units for input and output. Physics runs
//! in metres, kilograms and seconds: [`RagdollSpec::units_per_meter`]
//! defaults to 128 / 1.8 ≈ 71.1, so a 128-unit figure falls like a 1.8 m,
//! 80 kg person under 9.81 m/s². Shapes are fractions of the figure
//! height, so any skeleton size works.
//!
//! # Bodies, shapes, joints
//!
//! [`RagdollSpec::humanoid`] maps `Bip01` bone names (with fallbacks) to
//! 14 bodies: pelvis, abdomen, chest, head, upper arms, forearms (the
//! hands ride on them), thighs, calves, feet. Each body's segment runs
//! from its bone to the next body's bone in the rest pose (or a fallback length along a fixed
//! axis when that bone is missing or too close), and carries a capsule,
//! box or ball. Shoulders, hips, neck and spine are ball joints with
//! separate flex, side and twist limits; elbows, knees and ankles are
//! hinges, elbows and knees one-sided so they never bend backwards. Joint
//! frames come from the rest pose, so limits are anatomical whatever the
//! start pose. Hinges hold their ranges to within a degree or two; ball
//! joints are limited per axis, which the solver only approximates when
//! two axes are bent far at once, so a shoulder flung over the head can
//! pass a range by up to ~35° for a few frames on impact (the corpse is
//! always back inside). For the first [`Impulse::tone`] seconds a second,
//! motorized joint per pair holds the start pose's angles and lets go
//! linearly (muscle tone: the body topples before it goes limp); a little
//! joint damping stays afterwards ([`RagdollSpec::residual_tone`]). Every
//! bone that is not a body (fingers, toes, twist and helper bones, the
//! accumulation root) follows its nearest body ancestor rigidly, keeping
//! its offset from the start pose; bones with no body above them (the
//! accumulation root, helpers) follow the pelvis, except a skeleton root
//! (the scene root), which stays where the figure stood.
//!
//! Skeletons the humanoid map does not fit fail with
//! [`RagdollError::MissingBones`]; [`RagdollSpec::generic`] builds a
//! capped chain ragdoll from any bone tree instead (creatures).
//!
//! # Determinism
//!
//! Guaranteed, and tested: the same skeleton, pose, spec, impulse, seed,
//! duration and rate give a bit-identical clip on every run, in every
//! thread and every process of one build on one machine (tests: two runs
//! compared bit for bit for every preset; two threads against the main
//! thread; a golden FNV digest of every output float, which also pins
//! separate processes and both the dev and release profiles; different
//! seeds differ visibly). The means:
//! rapier's single-threaded pipeline (no `parallel` feature) with
//! `enhanced-determinism` (libm transcendentals, no 8-lane SIMD), bodies,
//! colliders and joints inserted in spec order, no hash-ordered iteration
//! in this crate, no clock, and one seeded generator ([`SplitMix64`]).
//! This crate's own arithmetic uses only `+ − × ÷` and `sqrt`, which IEEE
//! 754 fixes exactly; Rust never fuses or reorders float operations, so
//! the optimization level does not change a bit (the golden-hash test
//! holds in dev and release builds).
//!
//! Across CPU architectures and operating systems it is *expected but not
//! tested here*: rapier states bitwise cross-platform results under
//! `enhanced-determinism` and checks a golden hash on several targets in
//! its own CI; this crate adds nothing platform-dependent. A different
//! rapier, parry or glam version may change results; the golden-hash test
//! catches that.
//!
//! # Robustness
//!
//! Inputs are checked (finite pose, one matrix per bone, sane spec
//! numbers); missing optional bones are skipped and their children
//! re-parented; zero-length bones fall back to a fixed length; the number
//! of steps is capped ([`RagdollSpec::max_steps`]); a run that explodes
//! (non-finite state, a body too fast or too far, a joint pulled apart) is
//! retried twice with more substeps, more damping and a softer push (the
//! last retry also without self-collision) before
//! [`RagdollError::Exploded`]. A body still moving at the end of the
//! duration is damped to rest past it and the last pose replaced
//! ([`RagdollClip::forced`]).

mod geom;
mod impulse;
mod rig;
mod sim;
mod spec;

pub mod fixture;

#[cfg(test)]
mod tests;

use std::fmt;

pub use impulse::{Impulse, Preset, Push, SplitMix64};
pub use sim::{RagdollClip, simulate};
pub use spec::{
    Axis, BodySpec, DEFAULT_UNITS_PER_METER, Fallback, JointKind, JointSpec, MAX_BODIES,
    RagdollSpec, Segment, ShapeSpec,
};

/// Why a ragdoll could not be simulated.
#[derive(Debug, Clone, PartialEq)]
pub enum RagdollError {
    /// A spec number is out of range.
    InvalidSpec(String),
    /// The skeleton, pose, impulse, duration or rate is unusable.
    InvalidInput(String),
    /// Required bodies (by key) whose bones the skeleton lacks: it is not
    /// a humanoid the spec fits ([`RagdollSpec::generic`] fits any tree).
    MissingBones {
        /// The body keys.
        bodies: Vec<String>,
    },
    /// The skeleton cannot carry a ragdoll (no height, degenerate bones,
    /// fewer than two bodies).
    NotRiggable(String),
    /// Every attempt blew up.
    Exploded {
        /// Attempts made.
        attempts: u32,
        /// What went wrong on the last one.
        reason: String,
    },
}

impl fmt::Display for RagdollError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RagdollError::InvalidSpec(why) => write!(f, "ragdoll spec: {why}"),
            RagdollError::InvalidInput(why) => write!(f, "ragdoll input: {why}"),
            RagdollError::MissingBones { bodies } => write!(
                f,
                "the skeleton lacks the bones of required bodies: {}",
                bodies.join(", ")
            ),
            RagdollError::NotRiggable(why) => write!(f, "no ragdoll fits the skeleton: {why}"),
            RagdollError::Exploded { attempts, reason } => {
                write!(f, "the simulation blew up {attempts} times; last: {reason}")
            }
        }
    }
}

impl std::error::Error for RagdollError {}
