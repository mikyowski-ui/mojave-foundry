//! Critter animation codes, weapon codes, their file suffixes, and which
//! of them a critter needs to be attacked, shot and killed.
//!
//! Sources: the suffix codec is `vault_formats::fid::critter_suffix`
//! (SPEC-007 §2.2), and every name and rule below is from SPEC-007 §2.2,
//! §3.8, §3.10 and §3.11, SPEC-009 §3.9 and SPEC-013 §3.23. A critter FID
//! holds the animation in bits 16 to 23 and the weapon code in bits 12 to
//! 15; the file is `<stem><two letters>.frm` ([`Anim::suffix`]).
//!
//! The specifications name codes 2, 3 and 5 to 9 together ("jump,
//! falling, stairs", never started by the engine; SPEC-007 §3.11.3) and
//! name 5 alone as falling; the others keep neutral names here.

use crate::frm::FrmSpec;
use crate::{EmitError, critter::still_frm};
use std::collections::BTreeMap;
use vault_formats::fid::{ALIASED_ANIMATIONS, critter_suffix};

/// The critter weapon-animation code (FID bits 12 to 15; the weapon
/// prototype's animation word, SPEC-003 §2.3). Codes 1 to 10 give the
/// weapon letters `d` to `m` (SPEC-007 §2.2). These are the weapon classes
/// the art can show.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WeaponCode {
    /// No weapon animation (bare hands, and items held without one).
    #[default]
    Unarmed = 0,
    Knife = 1,
    Club = 2,
    Hammer = 3,
    Spear = 4,
    Pistol = 5,
    Smg = 6,
    Shotgun = 7,
    LaserRifle = 8,
    Minigun = 9,
    Launcher = 10,
}

impl WeaponCode {
    /// Every code, in order.
    pub const ALL: [WeaponCode; 11] = [
        Self::Unarmed,
        Self::Knife,
        Self::Club,
        Self::Hammer,
        Self::Spear,
        Self::Pistol,
        Self::Smg,
        Self::Shotgun,
        Self::LaserRifle,
        Self::Minigun,
        Self::Launcher,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    /// The weapon letter, `d` to `m`; `None` for code 0.
    pub fn letter(self) -> Option<u8> {
        (self != Self::Unarmed).then_some(b'c' + self as u8)
    }
}

/// The engine's critter animation codes 0 to 64 (SPEC-007 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Anim {
    /// 0 `aa` (weapon letter, `a`): standing, also the idle fidget.
    Stand = 0,
    /// 1 `ab` (weapon letter, `b`): walking.
    Walk = 1,
    /// 2 `ac`: script-only.
    Code2 = 2,
    /// 3 `ad`: script-only.
    Code3 = 3,
    /// 4 `ae`: climbing a ladder.
    ClimbLadder = 4,
    /// 5 `af`: falling (script-only).
    Falling = 5,
    /// 6 `ag`: script-only.
    Code6 = 6,
    /// 7 `ah`: script-only.
    Code7 = 7,
    /// 8 `ai`: script-only.
    Code8 = 8,
    /// 9 `aj`: script-only.
    Code9 = 9,
    /// 10 `ak`: reach to the ground (pick up, ground containers).
    GroundReach = 10,
    /// 11 `al`: middle-height reach (use).
    MiddleReach = 11,
    /// 12 `am`: high reach, never started by the engine.
    HighReach = 12,
    /// 13 `an` (weapon letter, `e`): dodge.
    Dodge = 13,
    /// 14 `ao`: hit from the front.
    HitFront = 14,
    /// 15 `ap`: hit from behind.
    HitBack = 15,
    /// 16 `aq`: punch.
    Punch = 16,
    /// 17 `ar`: kick.
    Kick = 17,
    /// 18 `as` (`dm` knife, `gm` spear): throw.
    Throw = 18,
    /// 19 `at`: running (any weapon code).
    Run = 19,
    /// 20 `ba`: fall back (knockdown, ordinary death).
    FallBack = 20,
    /// 21 `bb`: fall front.
    FallFront = 21,
    /// 22 `bc`: bad landing.
    BadLanding = 22,
    /// 23 `bd`: big hole.
    BigHole = 23,
    /// 24 `be`: charred body.
    Charred = 24,
    /// 25 `bf`: chunks.
    Chunks = 25,
    /// 26 `bg`: autofire dance.
    AutofireDance = 26,
    /// 27 `bh`: electrify (aliased).
    Electrify = 27,
    /// 28 `bi`: sliced in half.
    SlicedInHalf = 28,
    /// 29 `bj`: burned to nothing (aliased).
    BurnedToNothing = 29,
    /// 30 `bk`: electrified to nothing (aliased).
    ElectrifiedToNothing = 30,
    /// 31 `bl`: exploded to nothing.
    ExplodedToNothing = 31,
    /// 32 `bm`: melted to nothing.
    Melted = 32,
    /// 33 `bn`: fire dance (aliased).
    FireDance = 33,
    /// 34 `bo`: bloody fall back.
    BloodyFallBack = 34,
    /// 35 `bp`: bloody fall front.
    BloodyFallFront = 35,
    /// 36 `ch`: prone to standing (after a fall front).
    GetUpFront = 36,
    /// 37 `cj`: back to standing (after a fall back).
    GetUpBack = 37,
    /// 38 weapon letter `c`: take out the weapon.
    TakeOut = 38,
    /// 39 weapon letter `d`: put the weapon away.
    PutAway = 39,
    /// 40 weapon letter `e`: parry.
    Parry = 40,
    /// 41 weapon letter `f`: thrust.
    Thrust = 41,
    /// 42 weapon letter `g`: swing.
    Swing = 42,
    /// 43 weapon letter `h`: point (aim) before a ranged attack.
    Point = 43,
    /// 44 weapon letter `i`: unpoint after it.
    Unpoint = 44,
    /// 45 weapon letter `j`: single fire.
    SingleFire = 45,
    /// 46 weapon letter `k`: burst.
    Burst = 46,
    /// 47 weapon letter `l`: continuous fire.
    ContinuousFire = 47,
    /// 48 `ra`: lying after a fall back (static corpse).
    DeadFallBack = 48,
    /// 49 `rb`: lying after a fall front.
    DeadFallFront = 49,
    /// 50 `rc`.
    DeadBadLanding = 50,
    /// 51 `rd`.
    DeadBigHole = 51,
    /// 52 `re`.
    DeadCharred = 52,
    /// 53 `rf`.
    DeadChunks = 53,
    /// 54 `rg`.
    DeadAutofireDance = 54,
    /// 55 `rh` (aliased).
    DeadElectrify = 55,
    /// 56 `ri`.
    DeadSlicedInHalf = 56,
    /// 57 `rj` (aliased).
    DeadBurnedToNothing = 57,
    /// 58 `rk` (aliased).
    DeadElectrifiedToNothing = 58,
    /// 59 `rl`.
    DeadExplodedToNothing = 59,
    /// 60 `rm`.
    DeadMelted = 60,
    /// 61 `rn`.
    DeadFireDance = 61,
    /// 62 `ro`.
    DeadBloodyFallBack = 62,
    /// 63 `rp`.
    DeadBloodyFallFront = 63,
    /// 64 `na` (aliased): the called-shot window's picture (SPEC-014).
    CalledShot = 64,
}

impl Anim {
    /// Every code, 0 to 64.
    pub fn all() -> impl Iterator<Item = Anim> {
        (0..=64).filter_map(Self::from_code)
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        use Anim::*;
        const TABLE: [Anim; 65] = [
            Stand,
            Walk,
            Code2,
            Code3,
            ClimbLadder,
            Falling,
            Code6,
            Code7,
            Code8,
            Code9,
            GroundReach,
            MiddleReach,
            HighReach,
            Dodge,
            HitFront,
            HitBack,
            Punch,
            Kick,
            Throw,
            Run,
            FallBack,
            FallFront,
            BadLanding,
            BigHole,
            Charred,
            Chunks,
            AutofireDance,
            Electrify,
            SlicedInHalf,
            BurnedToNothing,
            ElectrifiedToNothing,
            ExplodedToNothing,
            Melted,
            FireDance,
            BloodyFallBack,
            BloodyFallFront,
            GetUpFront,
            GetUpBack,
            TakeOut,
            PutAway,
            Parry,
            Thrust,
            Swing,
            Point,
            Unpoint,
            SingleFire,
            Burst,
            ContinuousFire,
            DeadFallBack,
            DeadFallFront,
            DeadBadLanding,
            DeadBigHole,
            DeadCharred,
            DeadChunks,
            DeadAutofireDance,
            DeadElectrify,
            DeadSlicedInHalf,
            DeadBurnedToNothing,
            DeadElectrifiedToNothing,
            DeadExplodedToNothing,
            DeadMelted,
            DeadFireDance,
            DeadBloodyFallBack,
            DeadBloodyFallFront,
            CalledShot,
        ];
        TABLE.get(usize::from(code)).copied()
    }

    /// The two-letter file suffix for this animation with `weapon`,
    /// through `critter_suffix`. Refused for the weapon actions 38 to 47
    /// with [`WeaponCode::Unarmed`].
    pub fn suffix(self, weapon: WeaponCode) -> Result<[u8; 2], EmitError> {
        critter_suffix(self.code(), weapon.code())
            .map_err(|error| EmitError::Critter(error.to_string()))
    }

    /// [`Anim::suffix`] as text.
    pub fn suffix_str(self, weapon: WeaponCode) -> Result<String, EmitError> {
        let [a, b] = self.suffix(weapon)?;
        Ok(format!("{}{}", char::from(a), char::from(b)))
    }

    /// Whether the suffix depends on the weapon code: stand, walk, dodge,
    /// throw (knife and spear) and the weapon actions 38 to 47.
    pub fn uses_weapon_letter(self) -> bool {
        matches!(self.code(), 0 | 1 | 13 | 18 | 38..=47)
    }

    /// The weapon code under which this animation's file is kept: `weapon`
    /// when the suffix depends on it, else 0. A throw has its own file
    /// only for the knife and the spear (`dm`, `gm`); every other code
    /// throws with `as`.
    pub fn file_weapon(self, weapon: WeaponCode) -> WeaponCode {
        match self {
            Self::Throw if matches!(weapon, WeaponCode::Knife | WeaponCode::Spear) => weapon,
            Self::Throw => WeaponCode::Unarmed,
            _ if self.uses_weapon_letter() => weapon,
            _ => WeaponCode::Unarmed,
        }
    }

    /// Codes 38 to 47, which exist only with a weapon code.
    pub fn needs_weapon(self) -> bool {
        (38..=47).contains(&self.code())
    }

    /// Whether the file name takes the critter-list row's alias stem
    /// (`ALIASED_ANIMATIONS`).
    pub fn aliased(self) -> bool {
        ALIASED_ANIMATIONS.contains(&self.code())
    }

    /// Falls and deaths 20 to 35, except the fire dance, may be stored as
    /// per-direction `.fr0` to `.fr5` files (SPEC-007 §2.3); everything
    /// else is always one six-direction `.frm`.
    pub fn may_use_direction_files(self) -> bool {
        (20..=35).contains(&self.code()) && self != Self::FireDance
    }

    /// The figure ends on the ground: falls and deaths 20 to 35 and the
    /// static frames 48 to 63.
    pub fn ends_prone(self) -> bool {
        matches!(self.code(), 20..=35 | 48..=63)
    }

    /// Animations that loop in place or travel hex by hex: stand, walk,
    /// run. Every other animation plays once.
    pub fn loops(self) -> bool {
        matches!(self, Self::Stand | Self::Walk | Self::Run)
    }

    /// For a fall or death 20 to 35, its static last-frame counterpart 48
    /// to 63 (SPEC-007 §2.2).
    pub fn corpse(self) -> Option<Anim> {
        (20..=35)
            .contains(&self.code())
            .then(|| Self::from_code(self.code() + 28))
            .flatten()
    }
}

/// How the engine treats a missing animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    /// Without it something breaks: the critter is invisible, an attack on
    /// it or by it is thrown away, or it cannot get up.
    Required,
    /// The engine falls back to another animation or skips a step; the
    /// critter still works.
    Recommended,
    /// Only shown when the art exists.
    Optional,
}

/// One animation file a critter should have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    pub anim: Anim,
    pub weapon: WeaponCode,
    pub need: Need,
    /// What happens without it, and the rule's source.
    pub without: &'static str,
}

impl Requirement {
    /// The file suffix.
    pub fn suffix(&self) -> Result<String, EmitError> {
        self.anim.suffix_str(self.weapon)
    }
}

/// The attack modes of a weapon (the extended-flag nibbles, SPEC-003 §2.6).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum AttackMode {
    #[default]
    None = 0,
    Punch = 1,
    Kick = 2,
    Swing = 3,
    Thrust = 4,
    Throw = 5,
    Single = 6,
    Burst = 7,
    Continuous = 8,
}

impl AttackMode {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        use AttackMode::*;
        [
            None, Punch, Kick, Swing, Thrust, Throw, Single, Burst, Continuous,
        ]
        .get(usize::from(code))
        .copied()
    }

    /// The attack animation the mode plays (SPEC-007 §3.10.2).
    pub fn animation(self) -> Option<Anim> {
        Some(match self {
            Self::None => return None,
            Self::Punch => Anim::Punch,
            Self::Kick => Anim::Kick,
            Self::Swing => Anim::Swing,
            Self::Thrust => Anim::Thrust,
            Self::Throw => Anim::Throw,
            Self::Single => Anim::SingleFire,
            Self::Burst => Anim::Burst,
            Self::Continuous => Anim::ContinuousFire,
        })
    }

    /// Single, burst and continuous fire: the ranged modes, which point
    /// before the attack and unpoint after it.
    pub fn is_ranged_fire(self) -> bool {
        matches!(self, Self::Single | Self::Burst | Self::Continuous)
    }
}

const R: Need = Need::Required;
const C: Need = Need::Recommended;
const O: Need = Need::Optional;

/// What every critter that can be attacked, shot and killed needs, with
/// weapon code 0. Gore deaths 22 to 33 and their static frames 50 to 61
/// are optional: a death the art lacks falls back to 20 (SPEC-007 §3.8).
pub fn unarmed_requirements() -> Vec<Requirement> {
    let w = WeaponCode::Unarmed;
    let mut out = vec![
        req(
            Anim::Stand,
            w,
            R,
            "invisible (SPEC-007 §2.2; contracts 10.1)",
        ),
        req(
            Anim::Walk,
            w,
            R,
            "invisible while walking (SPEC-007 §3.11.1)",
        ),
        req(
            Anim::Dodge,
            w,
            R,
            "a missed shot from a weapon without a projectile registers the dodge without \
             testing the art, and the whole attack fails (SPEC-007 §3.10.4 step 7)",
        ),
        req(
            Anim::HitFront,
            w,
            R,
            "the hit flinch fails to register and the attack is thrown away (SPEC-007 \
             §3.10.1, §3.10.6)",
        ),
        req(
            Anim::HitBack,
            w,
            C,
            "a hit from behind flinches with 14 instead (SPEC-007 §3.10.6)",
        ),
        req(
            Anim::Punch,
            w,
            R,
            "cannot fight back unarmed (SPEC-009 §3.12)",
        ),
        req(
            Anim::Kick,
            w,
            C,
            "a punch is never turned into a kick (SPEC-007 §3.6)",
        ),
        req(
            Anim::FallBack,
            w,
            R,
            "knockdown and the ordinary death fail (SPEC-007 §3.8)",
        ),
        req(
            Anim::FallFront,
            w,
            R,
            "the engine falls back to 20 when choosing, but a knockdown from behind \
             registers 21 (SPEC-007 §3.8, §3.10.6; contracts 10.1)",
        ),
        req(
            Anim::GetUpBack,
            w,
            R,
            "stays down after a fall back (SPEC-007 §3.10.6)",
        ),
        req(
            Anim::GetUpFront,
            w,
            R,
            "stays down after a fall front (SPEC-007 §3.10.6)",
        ),
        req(
            Anim::DeadFallBack,
            w,
            R,
            "no body after an instant death or on map load (SPEC-007 §3.8, contracts 10.1)",
        ),
        req(Anim::DeadFallFront, w, R, "as 48, for a fall front"),
        req(Anim::Run, w, C, "walks instead (SPEC-007 §3.11.1)"),
        req(
            Anim::GroundReach,
            w,
            C,
            "picking up a weapon or using a ground container fails (SPEC-007 §3.11.3, \
             SPEC-009 §3.9)",
        ),
        req(
            Anim::MiddleReach,
            w,
            C,
            "using scenery or a container fails (SPEC-007 §3.11.3)",
        ),
        req(
            Anim::BloodyFallBack,
            w,
            O,
            "no blood after a fall back (SPEC-007 §3.10.6)",
        ),
        req(Anim::BloodyFallFront, w, O, "no blood after a fall front"),
        req(
            Anim::CalledShot,
            w,
            O,
            "the called-shot window shows its background (SPEC-014)",
        ),
        req(
            Anim::ClimbLadder,
            w,
            O,
            "only the player climbs (SPEC-013 §3.23)",
        ),
    ];
    for code in 22..=33 {
        let anim = Anim::from_code(code).expect("codes 22 to 33 exist");
        out.push(req(
            anim,
            w,
            O,
            "the death falls back to 20 (SPEC-007 §3.8)",
        ));
        if let Some(corpse) = anim.corpse() {
            out.push(req(
                corpse,
                w,
                O,
                "a direct death falls back to 20 (SPEC-007 §3.8)",
            ));
        }
    }
    out
}

/// What a critter holding a weapon with `code` and `modes` needs on top
/// of [`unarmed_requirements`]. The critter's art carries the weapon code
/// while it holds the weapon, so stand, walk and dodge need the lettered
/// files; SPEC-009 §3.9 treats a weapon as unusable unless the art for its
/// mode's animation exists.
pub fn weapon_requirements(code: WeaponCode, modes: &[AttackMode]) -> Vec<Requirement> {
    let mut out = Vec::new();
    if code != WeaponCode::Unarmed {
        out.extend([
            req(Anim::Stand, code, R, "invisible while holding the weapon"),
            req(Anim::Walk, code, R, "invisible while walking with it"),
            req(
                Anim::Dodge,
                code,
                R,
                "as the unarmed dodge, while holding it (SPEC-007 §3.10.4)",
            ),
            req(
                Anim::TakeOut,
                code,
                R,
                "wielding it fails, so the AI cannot switch to it (SPEC-007 §3.10.1, \
                 SPEC-009 §3.9)",
            ),
            req(
                Anim::PutAway,
                code,
                C,
                "using a container or scenery while armed fails (SPEC-007 §3.11.3)",
            ),
        ]);
    }
    for mode in modes {
        let Some(anim) = mode.animation() else {
            continue;
        };
        if anim.needs_weapon() && code == WeaponCode::Unarmed {
            continue;
        }
        out.push(req(
            anim,
            code,
            R,
            "the weapon is unusable in this mode (SPEC-009 §3.9)",
        ));
        if mode.is_ranged_fire() && code != WeaponCode::Unarmed {
            out.push(req(
                Anim::Point,
                code,
                R,
                "a ranged attack points first; without the art it fails (SPEC-007 §3.10.4)",
            ));
            out.push(req(
                Anim::Unpoint,
                code,
                R,
                "a ranged attack unpoints after (SPEC-007 §3.10.4)",
            ));
        }
    }
    out
}

/// [`unarmed_requirements`] plus [`weapon_requirements`] for each weapon,
/// one entry per file, the strongest need kept, ordered by weapon code and
/// animation.
pub fn combat_requirements(weapons: &[(WeaponCode, [AttackMode; 2])]) -> Vec<Requirement> {
    let mut all = unarmed_requirements();
    for (code, modes) in weapons {
        all.extend(weapon_requirements(*code, modes));
    }
    let mut by_file: BTreeMap<(WeaponCode, Anim), Requirement> = BTreeMap::new();
    for requirement in all {
        // Codes whose suffix ignores the weapon share one file: keep them
        // under weapon code 0.
        let weapon = requirement.anim.file_weapon(requirement.weapon);
        let requirement = Requirement {
            weapon,
            ..requirement
        };
        by_file
            .entry((weapon, requirement.anim))
            .and_modify(|kept| {
                if requirement.need < kept.need {
                    *kept = requirement;
                }
            })
            .or_insert(requirement);
    }
    by_file.into_values().collect()
}

fn req(anim: Anim, weapon: WeaponCode, need: Need, without: &'static str) -> Requirement {
    Requirement {
        anim,
        weapon,
        need,
        without,
    }
}

/// A critter's animation files, keyed by animation and weapon code.
///
/// Animations whose suffix ignores the weapon code are kept under weapon
/// code 0 ([`Anim::file_weapon`]): the engine asks for the same file
/// whatever the critter holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnimationSet {
    frms: BTreeMap<(WeaponCode, Anim), FrmSpec>,
}

impl AnimationSet {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(anim: Anim, weapon: WeaponCode) -> Result<(WeaponCode, Anim), EmitError> {
        anim.suffix(weapon)?;
        Ok((anim.file_weapon(weapon), anim))
    }

    /// Add or replace an animation. A critter animation has six
    /// directions.
    pub fn insert(
        &mut self,
        anim: Anim,
        weapon: WeaponCode,
        frm: FrmSpec,
    ) -> Result<(), EmitError> {
        if frm.directions.len() != 6 {
            return Err(EmitError::Critter(format!(
                "a critter animation has 6 directions, not {}",
                frm.directions.len()
            )));
        }
        self.frms.insert(Self::key(anim, weapon)?, frm);
        Ok(())
    }

    pub fn get(&self, anim: Anim, weapon: WeaponCode) -> Option<&FrmSpec> {
        Self::key(anim, weapon)
            .ok()
            .and_then(|key| self.frms.get(&key))
    }

    pub fn contains(&self, anim: Anim, weapon: WeaponCode) -> bool {
        self.get(anim, weapon).is_some()
    }

    pub fn len(&self) -> usize {
        self.frms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frms.is_empty()
    }

    /// The requirements at `need` or stronger that the set lacks.
    pub fn missing(&self, requirements: &[Requirement], need: Need) -> Vec<Requirement> {
        requirements
            .iter()
            .filter(|r| r.need <= need && !self.contains(r.anim, r.weapon))
            .copied()
            .collect()
    }

    /// Fill every missing requirement at `need` or stronger with a still
    /// ([`still_frm`]) of the stand for its weapon code, or of the unarmed
    /// stand when that is missing too: the first wave's placeholders
    /// (contracts 10.4). Returns what was filled. Refused when the set has
    /// no unarmed stand.
    pub fn fill_placeholders(
        &mut self,
        requirements: &[Requirement],
        need: Need,
    ) -> Result<Vec<Requirement>, EmitError> {
        let missing = self.missing(requirements, need);
        let unarmed = self
            .get(Anim::Stand, WeaponCode::Unarmed)
            .cloned()
            .ok_or_else(|| EmitError::Critter("the set has no unarmed stand (aa)".into()))?;
        for requirement in &missing {
            let stand = self
                .get(Anim::Stand, requirement.weapon)
                .unwrap_or(&unarmed);
            let still = still_frm(stand)?;
            self.insert(requirement.anim, requirement.weapon, still)?;
        }
        Ok(missing)
    }

    /// Every file as `(suffix, frm)`, in suffix order: what the map
    /// builder's per-critter art map keys by suffix.
    pub fn by_suffix(&self) -> Result<BTreeMap<String, &FrmSpec>, EmitError> {
        let mut out = BTreeMap::new();
        for ((weapon, anim), frm) in &self.frms {
            out.insert(anim.suffix_str(*weapon)?, frm);
        }
        Ok(out)
    }

    /// Every file name, `<stem><suffix>.frm`, with its sprite.
    pub fn files(&self, stem: &str) -> Result<BTreeMap<String, &FrmSpec>, EmitError> {
        Ok(self
            .by_suffix()?
            .into_iter()
            .map(|(suffix, frm)| (format!("{stem}{suffix}.frm"), frm))
            .collect())
    }

    /// The entries, by weapon code then animation.
    pub fn iter(&self) -> impl Iterator<Item = (Anim, WeaponCode, &FrmSpec)> {
        self.frms
            .iter()
            .map(|((weapon, anim), frm)| (*anim, *weapon, frm))
    }
}
