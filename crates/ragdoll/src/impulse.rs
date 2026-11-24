//! The kill push, its presets, and the seeded perturbation.

/// A tiny seeded generator (SplitMix64). The only source of variety in a
/// simulation: no clock, no thread-local or OS randomness.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A generator started from `seed`.
    pub fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `[-1, 1)`, exactly representable (24 random bits).
    pub fn signed(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32; // 24 bits
        (bits as f32) / 8_388_608.0 - 1.0
    }
}

/// One push on one body, or on every body at once ([`Push::EVERY_BODY`]).
///
/// Directions are in the figure's own frame: `[right, forward, up]`
/// ([`crate::RagdollSpec::up`] and [`crate::RagdollSpec::forward`]), so a
/// push is baked once and the clip turned to each of the six facings.
#[derive(Debug, Clone, PartialEq)]
pub struct Push {
    /// Candidate body keys, first present wins; the root when none is.
    /// [`Push::EVERY_BODY`] pushes every body instead, each in proportion
    /// to its mass and its height above the ground (twice `speed` at half
    /// the figure's height): the whole figure starts to topple about its
    /// feet as one piece, which a push on one body cannot do without
    /// bending it.
    pub bodies: Vec<String>,
    /// Where along the body's segment the push lands: 0 at the bone
    /// origin (the joint), 1 at the far end.
    pub along: f32,
    /// Direction `[right, forward, up]`; normalized before use.
    pub direction: [f32; 3],
    /// Strength as a velocity change of the whole figure, m/s: the impulse
    /// is this times the total mass, so presets do not depend on mass.
    pub speed: f32,
    /// When the push starts, seconds after the start pose.
    pub start: f32,
    /// How long it is spread over, seconds (0: one step).
    pub duration: f32,
}

impl Push {
    /// The body key that means every body ([`Push::bodies`]).
    pub const EVERY_BODY: &'static str = "*";
}

/// The kill push: one or more [`Push`]es plus a seeded perturbation.
#[derive(Debug, Clone, PartialEq)]
pub struct Impulse {
    /// The pushes, applied in order.
    pub pushes: Vec<Push>,
    /// Perturbation strength in `[0, 1]`: each push's direction gets a
    /// random offset of up to this length (before normalizing) and its
    /// speed a random change of up to ±`jitter` × 50 %. 0 makes every seed
    /// give the same fall.
    pub jitter: f32,
    /// Seconds of muscle tone: every joint starts holding its start-pose
    /// angles ([`crate::RagdollSpec::tone_stiffness`]) and lets go linearly
    /// over this time, so the body topples before it goes limp. 0 is limp
    /// from the first step.
    pub tone: f32,
}

/// Ready-made kill pushes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Preset {
    /// Shot from the front: the whole figure is tipped backwards (0.5 m/s
    /// at mid-height) and topples, stiff for 0.5 s, onto its back about
    /// one body length behind its feet.
    FallBackward,
    /// Shot from behind: tipped forwards harder (1 m/s; a standing figure's
    /// weight sits over its heels) and lands face down.
    FallForward,
    /// The legs give out (knees pushed forward, pelvis down, 0.15 s of
    /// tone): the body drops near where it stood, usually onto its back.
    Crumple,
    /// An explosion in front: the figure is thrown back as one piece and
    /// lifted at the pelvis; it lands on its back well behind its feet.
    ThrownBack,
}

impl Preset {
    /// Every preset, in a fixed order.
    pub const ALL: [Preset; 4] = [
        Preset::FallBackward,
        Preset::FallForward,
        Preset::Crumple,
        Preset::ThrownBack,
    ];

    /// A short lowercase name (`fall_backward`, ...).
    pub fn name(self) -> &'static str {
        match self {
            Preset::FallBackward => "fall_backward",
            Preset::FallForward => "fall_forward",
            Preset::Crumple => "crumple",
            Preset::ThrownBack => "thrown_back",
        }
    }

    /// The Fallout 2 animation suffix this preset bakes
    /// (`docs/contracts.md` 10.1): `ba` (fall back, code 20) and `bb`
    /// (fall front, code 21). The other presets are variants for either.
    pub fn fall_suffix(self) -> Option<&'static str> {
        match self {
            Preset::FallBackward => Some("ba"),
            Preset::FallForward => Some("bb"),
            Preset::Crumple | Preset::ThrownBack => None,
        }
    }

    /// The corpse still (codes 48 and 49) made from this preset's last
    /// pose: `ra` lying on the back, `rb` on the front.
    pub fn corpse_suffix(self) -> Option<&'static str> {
        match self {
            Preset::FallBackward => Some("ra"),
            Preset::FallForward => Some("rb"),
            Preset::Crumple | Preset::ThrownBack => None,
        }
    }
}

fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

impl Impulse {
    /// The push of a preset, with the default jitter (0.2).
    pub fn preset(preset: Preset) -> Self {
        let chest = || keys(&["chest", "abdomen", "pelvis"]);
        let push = |bodies, along, direction, speed, start, duration| Push {
            bodies,
            along,
            direction,
            speed,
            start,
            duration,
        };
        let knees = |speed: f32, forward: f32| {
            ["calf_l", "calf_r"]
                .map(|k| push(keys(&[k]), 0.0, [0.0, forward, 0.0], speed, 0.0, 0.1))
        };
        let pushes = match preset {
            Preset::FallBackward => {
                // A push on the chest alone folds the body at the hips and
                // the legs whip up on landing; tipping it whole keeps it
                // straight.
                vec![push(
                    keys(&[Push::EVERY_BODY]),
                    0.5,
                    [0.0, -1.0, 0.0],
                    0.5,
                    0.0,
                    0.1,
                )]
            }
            Preset::FallForward => {
                // The whole body topples forward as one piece; pushed at the
                // chest alone it folds at the hips and rolls over its head.
                vec![push(
                    keys(&[Push::EVERY_BODY]),
                    0.5,
                    [0.0, 1.0, 0.0],
                    1.0,
                    0.0,
                    0.1,
                )]
            }
            Preset::Crumple => {
                let mut v = vec![
                    push(keys(&["pelvis"]), 0.5, [0.0, 0.0, -1.0], 0.075, 0.0, 0.1),
                    push(chest(), 0.8, [0.0, 1.0, 0.0], 0.125, 0.0, 0.1),
                ];
                v.extend(knees(0.03, 1.0));
                v
            }
            Preset::ThrownBack => vec![
                // Thrown as one piece, turning back as it goes.
                push(
                    keys(&[Push::EVERY_BODY]),
                    0.5,
                    [0.0, -1.0, 0.0],
                    1.0,
                    0.0,
                    0.06,
                ),
                push(keys(&["pelvis"]), 0.5, [0.0, -0.5, 1.0], 0.8, 0.0, 0.06),
            ],
        };
        let tone = match preset {
            Preset::FallBackward | Preset::FallForward | Preset::ThrownBack => 0.5,
            Preset::Crumple => 0.15,
        };
        Impulse {
            pushes,
            jitter: 0.2,
            tone,
        }
    }

    /// No push at all: the body falls under gravity alone.
    pub fn none() -> Self {
        Impulse {
            pushes: Vec::new(),
            jitter: 0.0,
            tone: 0.0,
        }
    }

    /// The pushes with the seed's perturbation applied, directions
    /// normalized. Only `+ - * /` and `sqrt`, so the result is the same on
    /// every platform.
    pub(crate) fn perturbed(&self, seed: u64) -> Vec<Push> {
        let mut rng = SplitMix64::new(seed);
        let jitter = self.jitter.clamp(0.0, 1.0);
        self.pushes
            .iter()
            .map(|p| {
                let r = [rng.signed(), rng.signed(), rng.signed()];
                let s = rng.signed();
                let d: [f32; 3] = std::array::from_fn(|k| p.direction[k] + r[k] * jitter);
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let direction = if len.is_finite() && len > 1e-6 {
                    d.map(|v| v / len)
                } else {
                    [0.0; 3]
                };
                Push {
                    direction,
                    speed: p.speed * (1.0 + 0.5 * jitter * s),
                    ..p.clone()
                }
            })
            .collect()
    }

    /// Checks every number for sense.
    pub(crate) fn validate(&self) -> Result<(), crate::RagdollError> {
        let ok = self.jitter.is_finite()
            && (0.0..=1.0).contains(&self.jitter)
            && self.tone.is_finite()
            && (0.0..=10.0).contains(&self.tone)
            && self.pushes.iter().all(|p| {
                p.direction.iter().all(|v| v.is_finite())
                    && p.along.is_finite()
                    && p.speed.is_finite()
                    && p.speed.abs() <= 50.0
                    && p.start.is_finite()
                    && p.start >= 0.0
                    && p.duration.is_finite()
                    && p.duration >= 0.0
            });
        if ok {
            Ok(())
        } else {
            Err(crate::RagdollError::InvalidInput(
                "impulse: jitter must be within [0, 1], tone within [0, 10] s; push numbers finite, speed ≤ 50 m/s, times ≥ 0"
                    .into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_reproducible_and_bounded() {
        let mut a = SplitMix64::new(7);
        let mut b = SplitMix64::new(7);
        for _ in 0..1000 {
            let x = a.signed();
            assert_eq!(x.to_bits(), b.signed().to_bits());
            assert!((-1.0..1.0).contains(&x));
        }
        assert_ne!(SplitMix64::new(1).next_u64(), SplitMix64::new(2).next_u64());
    }

    #[test]
    fn perturbation_depends_on_the_seed_only() {
        let i = Impulse::preset(Preset::FallBackward);
        assert_eq!(i.perturbed(3), i.perturbed(3));
        assert_ne!(i.perturbed(3), i.perturbed(4));
        let calm = Impulse { jitter: 0.0, ..i };
        assert_eq!(calm.perturbed(3), calm.perturbed(4));
    }
}
