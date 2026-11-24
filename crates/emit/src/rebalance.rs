//! Rebalancing: per-game tunable tables from source-game figures to
//! Fallout 2 prototype values, and pure functions over small plain input
//! structs that the converter fills from its records.
//!
//! Nothing here reads a file. [`Rebalance::fallout3_default`] and
//! [`Rebalance::new_vegas_default`] are uncalibrated starting points,
//! chosen so the synthetic weapons, armour and people of the tests land in
//! the ranges the engine's rules make playable (AP 1 to 9, the interface's
//! range; DR within its clamp; skills that pass an AI packet's
//! `min_to_hit`). Every field is public to tune.
//!
//! Design facts behind the two defaults (no source-game code involved):
//! Fallout 3 armour is resistance-based (a damage-resistance percentage,
//! no threshold), New Vegas armour is threshold-based (a damage threshold
//! subtracted per hit, with resistance rare). So the Fallout 3 table
//! derives DR mostly from DR and a small DT from it, and the New Vegas
//! table derives DT from DT and DR from DT. New Vegas folds Small Guns and
//! Big Guns into one Guns skill.
//!
//! The Fallout 2 side follows the approved specifications: the skill a
//! weapon uses is implied by its attack mode, damage type and big-gun flag
//! ([`crate::items::implied_skill`]); calibres are compared for equality
//! only (SPEC-011 §3.15), so this crate numbers its own from 101 up, apart
//! from any base-game ammunition; a non-player critter's protection is its
//! prototype's (SPEC-011 §3.10), so [`fighter_spec`] folds worn armour
//! into it.

use crate::animation::{AttackMode, WeaponCode};
use crate::fighter::FighterSpec;
use crate::items::{
    AmmoSpec, ArmorSpec, DamageType, DrugSpec, DrugStat, Skill, WeaponSpec, armor_perk, drug_stat,
    weapon_perk, withdrawal_perk,
};
use crate::overlay::{Overlay, overlay_leaf, overlay_struct};
use crate::proto::{CritterSpec, ItemStats, stat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The game a record comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceGame {
    Fallout3,
    NewVegas,
}

/// The converter's classification of a source weapon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaponClass {
    Pistol,
    Smg,
    Rifle,
    AssaultRifle,
    Shotgun,
    Minigun,
    Flamer,
    Launcher,
    EnergyPistol,
    EnergyRifle,
    EnergyHeavy,
    Blade,
    Blunt,
    HeavyMelee,
    Spear,
    Unarmed,
    Thrown,
    Grenade,
}

impl WeaponClass {
    pub const ALL: [WeaponClass; 18] = [
        Self::Pistol,
        Self::Smg,
        Self::Rifle,
        Self::AssaultRifle,
        Self::Shotgun,
        Self::Minigun,
        Self::Flamer,
        Self::Launcher,
        Self::EnergyPistol,
        Self::EnergyRifle,
        Self::EnergyHeavy,
        Self::Blade,
        Self::Blunt,
        Self::HeavyMelee,
        Self::Spear,
        Self::Unarmed,
        Self::Thrown,
        Self::Grenade,
    ];
}

/// The kind of damage a source weapon deals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceDamage {
    #[default]
    Ballistic,
    Laser,
    Plasma,
    Fire,
    Electric,
    Pulse,
    Explosive,
}

/// A source weapon's figures, as the converter reads them. Zero means
/// "not given" wherever the field notes say so.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceWeapon {
    pub name: String,
    pub class: Option<WeaponClass>,
    /// Damage per projectile per shot.
    pub damage: f32,
    /// Projectiles per shot (shotgun pellets); 0 counts as 1.
    pub projectiles: u32,
    pub damage_kind: SourceDamage,
    /// Rounds per magazine.
    pub clip: u32,
    pub value: i32,
    pub weight: f32,
    /// The source's action-point cost of an attack; 0 when not given.
    pub ap: f32,
    /// Shots per second; 0 when not given.
    pub fire_rate: f32,
    pub automatic: bool,
    /// The source's minimum spread (lower is more accurate); 0 when not
    /// given.
    pub min_spread: f32,
    /// Effective range in source units; 0 when not given.
    pub range: f32,
    /// Required strength; 0 when none.
    pub strength: i32,
    /// The ammunition's calibre key ([`normalize_key`] is applied).
    pub caliber: Option<String>,
    pub two_handed: bool,
}

/// A source armour's figures.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceArmor {
    pub name: String,
    /// Damage resistance, percent.
    pub dr: f32,
    /// Damage threshold.
    pub dt: f32,
    pub value: i32,
    pub weight: f32,
    /// Powered armour: takes the Powered Armor perk.
    pub power_armor: bool,
}

/// A source ammunition's figures.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceAmmo {
    pub name: String,
    pub caliber: String,
    /// Value per round.
    pub value: f32,
    /// Weight per round.
    pub weight: f32,
    /// Damage multiplier; 0 counts as 1.
    pub damage_mult: f32,
    pub dr_modifier: i32,
    pub ac_modifier: i32,
    /// Rounds per pack; 0 takes [`AmmoRule::default_pack`].
    pub pack: u32,
}

/// A stat a source aid item changes, in Fallout 2 terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AidStat {
    /// Strength (0) to luck (6).
    Special(u8),
    ActionPoints,
    ArmorClass,
    MeleeDamage,
    CarryWeight,
    CriticalChance,
    HealingRate,
    /// Normal damage resistance.
    DamageResistance,
    RadiationResistance,
    PoisonResistance,
}

impl AidStat {
    /// The Fallout 2 stat index (SPEC-011 §2.1).
    pub fn index(self) -> u8 {
        match self {
            Self::Special(n) => n.min(6),
            Self::ActionPoints => stat::MAX_ACTION_POINTS as u8,
            Self::ArmorClass => stat::ARMOR_CLASS as u8,
            Self::MeleeDamage => stat::MELEE_DAMAGE as u8,
            Self::CarryWeight => stat::CARRY_WEIGHT as u8,
            Self::CriticalChance => stat::CRITICAL_CHANCE as u8,
            Self::HealingRate => stat::HEALING_RATE as u8,
            Self::DamageResistance => 24,
            Self::RadiationResistance => stat::RADIATION_RESISTANCE as u8,
            Self::PoisonResistance => stat::POISON_RESISTANCE as u8,
        }
    }
}

/// One timed effect of a source aid item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceEffect {
    pub stat: AidStat,
    pub amount: i32,
    /// How long it lasts, in source seconds; 0 is permanent.
    pub seconds: f32,
}

/// A source aid item (a chem, a medicine, food).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceAid {
    pub name: String,
    pub value: i32,
    pub weight: f32,
    /// Hit points restored, in total.
    pub health: i32,
    /// Radiation removed (positive) or given (negative).
    pub radiation: i32,
    pub effects: Vec<SourceEffect>,
    /// 0 to 1.
    pub addiction_chance: f32,
    /// A withdrawal perk selector ([`withdrawal_perk`]); `None` takes
    /// [`AidRule::withdrawal_perk`] when the item is addictive.
    pub withdrawal: Option<i32>,
}

/// A source skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSkill {
    Barter,
    BigGuns,
    EnergyWeapons,
    Explosives,
    Guns,
    Lockpick,
    Medicine,
    MeleeWeapons,
    Repair,
    Science,
    SmallGuns,
    Sneak,
    Speech,
    Survival,
    Unarmed,
}

/// A source actor's figures.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceActor {
    pub name: String,
    pub level: i32,
    pub health: i32,
    /// Strength to luck, the same 1 to 10 scale.
    pub special: [i32; 7],
    pub skills: Vec<(SourceSkill, i32)>,
    /// The source's action points; 0 when not given.
    pub action_points: i32,
    pub female: bool,
}

/// How one weapon class becomes a Fallout 2 weapon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClassRule {
    /// The art's weapon code.
    pub code: WeaponCode,
    /// Primary and secondary modes; an automatic source weapon whose
    /// class has a single-fire primary and no secondary gets burst.
    pub modes: [AttackMode; 2],
    pub big_gun: bool,
    pub two_handed: bool,
    /// Ranged fire with ballistic source damage is made laser, so the
    /// weapon uses Energy Weapons.
    pub energy: bool,
    /// Default range per mode, hexes.
    pub ranges: [i32; 2],
    pub min_strength: i32,
    pub size: i32,
    /// Rounds per burst without a source fire rate.
    pub burst: i32,
    pub critical_failure: i32,
}

impl Default for ClassRule {
    /// A one-handed single-shot pistol.
    fn default() -> Self {
        rule(
            WeaponCode::Pistol,
            [AttackMode::Single, AttackMode::None],
            [20, 0],
            3,
            2,
        )
    }
}

/// Damage scaling: a source shot's damage D (per projectile × projectiles)
/// becomes the range `[round(D × scale × spread[0]), round(D × scale ×
/// spread[1])]`, at least `min`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DamageRule {
    pub scale: f32,
    pub spread: [f32; 2],
    pub min: i32,
}

impl Default for DamageRule {
    fn default() -> Self {
        Self {
            scale: 1.0,
            spread: [0.6, 1.4],
            min: 1,
        }
    }
}

/// One AP figure per attack mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModeCosts {
    pub punch: i32,
    pub kick: i32,
    pub swing: i32,
    pub thrust: i32,
    pub throw: i32,
    pub single: i32,
    pub burst: i32,
    pub continuous: i32,
}

impl ModeCosts {
    /// The figure for `mode` (0 for none).
    pub fn get(&self, mode: AttackMode) -> i32 {
        match mode {
            AttackMode::None => 0,
            AttackMode::Punch => self.punch,
            AttackMode::Kick => self.kick,
            AttackMode::Swing => self.swing,
            AttackMode::Thrust => self.thrust,
            AttackMode::Throw => self.throw,
            AttackMode::Single => self.single,
            AttackMode::Burst => self.burst,
            AttackMode::Continuous => self.continuous,
        }
    }
}

impl Default for ModeCosts {
    /// The base costs: 3 for unarmed and melee, 4 to throw, 5 for a
    /// single shot, 6 for burst and continuous fire.
    fn default() -> Self {
        Self {
            punch: 3,
            kick: 3,
            swing: 3,
            thrust: 3,
            throw: 4,
            single: 5,
            burst: 6,
            continuous: 6,
        }
    }
}

/// AP costs: `base` by attack mode; with a source cost and a
/// `source_divisor` above 0, `round(source / divisor) + extra`. Clamped to
/// `min..=max` (the interface shows 0 to 9).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApRule {
    pub base: ModeCosts,
    pub source_divisor: f32,
    pub extra: ModeCosts,
    pub min: i32,
    pub max: i32,
}

impl Default for ApRule {
    fn default() -> Self {
        Self {
            base: ModeCosts::default(),
            source_divisor: 4.0,
            extra: ModeCosts {
                punch: 0,
                kick: 0,
                swing: 0,
                thrust: 0,
                throw: 0,
                single: 0,
                burst: 1,
                continuous: 1,
            },
            min: 1,
            max: 9,
        }
    }
}

/// Ranges: a source range over `units_per_hex`, at most `max`; the class
/// default without one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RangeRule {
    pub units_per_hex: f32,
    pub max: i32,
}

impl Default for RangeRule {
    fn default() -> Self {
        Self {
            units_per_hex: 64.0,
            max: 50,
        }
    }
}

/// Burst rounds: `round(fire_rate × seconds)`, clamped.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BurstRule {
    pub seconds: f32,
    pub min: i32,
    pub max: i32,
}

impl Default for BurstRule {
    fn default() -> Self {
        Self {
            seconds: 1.0,
            min: 3,
            max: 40,
        }
    }
}

/// One factor per damage type.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PerDamage {
    pub normal: f32,
    pub laser: f32,
    pub fire: f32,
    pub plasma: f32,
    pub electrical: f32,
    pub emp: f32,
    pub explosion: f32,
}

impl PerDamage {
    /// The factor for `kind`.
    pub fn get(&self, kind: DamageType) -> f32 {
        match kind {
            DamageType::Normal => self.normal,
            DamageType::Laser => self.laser,
            DamageType::Fire => self.fire,
            DamageType::Plasma => self.plasma,
            DamageType::Electrical => self.electrical,
            DamageType::Emp => self.emp,
            DamageType::Explosion => self.explosion,
        }
    }
}

impl Default for PerDamage {
    /// Normal 1, laser, fire and explosion 0.8, plasma and electrical 0.6,
    /// EMP 0 (EMP has its own figure).
    fn default() -> Self {
        Self {
            normal: 1.0,
            laser: 0.8,
            fire: 0.8,
            plasma: 0.6,
            electrical: 0.6,
            emp: 0.0,
            explosion: 0.8,
        }
    }
}

/// Armour: normal DR `dr × dr_scale + dt × dr_per_dt` (0 to 90), normal DT
/// `dt × dt_scale + dr × dt_per_dr` (0 to 100), AC `ac_base + dr ×
/// ac_per_dr + dt × ac_per_dt`; other damage types by the factors, EMP DR
/// `emp_dr` and EMP DT 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArmorRule {
    pub dr_scale: f32,
    pub dr_per_dt: f32,
    pub dt_scale: f32,
    pub dt_per_dr: f32,
    pub ac_base: f32,
    pub ac_per_dr: f32,
    pub ac_per_dt: f32,
    pub emp_dr: i32,
    pub size: i32,
    pub dr_factors: PerDamage,
    pub dt_factors: PerDamage,
}

impl Default for ArmorRule {
    /// The Fallout 3 (resistance-based) rule.
    fn default() -> Self {
        Self {
            dr_scale: 2.0,
            dr_per_dt: 0.0,
            dt_scale: 0.0,
            dt_per_dr: 0.15,
            ac_base: 0.0,
            ac_per_dr: 1.0,
            ac_per_dt: 0.0,
            emp_dr: 100,
            size: 10,
            dr_factors: PerDamage::default(),
            dt_factors: PerDamage::default(),
        }
    }
}

/// Ammunition: the pack size without a source one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AmmoRule {
    pub default_pack: i32,
}

impl Default for AmmoRule {
    fn default() -> Self {
        Self { default_pack: 20 }
    }
}

/// Aid items: hit points and radiation scaling, source seconds to game
/// minutes, and the withdrawal defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AidRule {
    pub health_scale: f32,
    pub radiation_scale: f32,
    pub minutes_per_second: f32,
    pub withdrawal_perk: i32,
    pub withdrawal_onset: i32,
}

impl Default for AidRule {
    fn default() -> Self {
        Self {
            health_scale: 0.5,
            radiation_scale: 0.5,
            minutes_per_second: 0.5,
            withdrawal_perk: withdrawal_perk::DESCRIPTION_ONLY,
            withdrawal_onset: 1440,
        }
    }
}

/// People: HP `health × hp_scale + level × hp_per_level` (at least 1); AP
/// `source / ap_divisor` when both are above 0, at most `ap_max`; skills
/// `× skill_scale`; experience `xp_base + level × xp_per_level`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CritterRule {
    pub hp_scale: f32,
    pub hp_per_level: f32,
    pub ap_divisor: f32,
    pub ap_max: i32,
    pub skill_scale: f32,
    pub xp_base: i32,
    pub xp_per_level: i32,
    pub age: i32,
}

impl Default for CritterRule {
    /// The Fallout 3 rule.
    fn default() -> Self {
        Self {
            hp_scale: 0.5,
            hp_per_level: 1.0,
            ap_divisor: 10.0,
            ap_max: 10,
            skill_scale: 1.0,
            xp_base: 25,
            xp_per_level: 10,
            age: 25,
        }
    }
}

/// The whole table set for one game.
///
/// **TOML.** Every type here is `Serialize` and `Deserialize` with
/// snake_case keys, `#[serde(default)]` and `deny_unknown_fields`: an
/// unknown key or a misspelt one is an error. Weapon classes, source
/// skills and calibres are table keys (`[classes.smg]`, `[skills]
/// guns = ["small_guns", "big_guns"]`, `[calibers] 10mm = 101`). A plain
/// `Deserialize` fills every missing key from the type's `Default`, which
/// is the **Fallout 3** table; to change a few keys over either game's
/// default, start from [`Rebalance::for_game`] and call
/// [`Rebalance::overlay`] with the file's deserializer, which replaces
/// only the keys the file gives (inside nested tables too) and keeps
/// every other value of that game's default. The example files
/// [`FALLOUT3_TOML`] and [`NEW_VEGAS_TOML`] are the two full defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rebalance {
    pub game: SourceGame,
    /// The class for a weapon without one.
    pub default_class: WeaponClass,
    /// The calibre of a weapon or ammunition whose key is unknown.
    pub unknown_caliber: i32,
    /// A ranged weapon whose source minimum spread is above 0 and below
    /// this takes the Accurate perk; absent (`None`) never.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accurate_spread_below: Option<f32>,
    pub weight_scale: f32,
    pub cost_scale: f32,
    pub damage: DamageRule,
    pub ap: ApRule,
    pub range: RangeRule,
    pub burst: BurstRule,
    pub armor: ArmorRule,
    pub ammo: AmmoRule,
    pub aid: AidRule,
    pub critter: CritterRule,
    pub classes: BTreeMap<WeaponClass, ClassRule>,
    /// Calibre keys ([`normalize_key`]) to numbers. Keys are normalised
    /// when looked up, so a file may write them as it likes.
    pub calibers: BTreeMap<String, i32>,
    /// Each source skill and the Fallout 2 skills it sets (the highest
    /// value wins).
    pub skills: BTreeMap<SourceSkill, Vec<Skill>>,
}

impl Default for Rebalance {
    /// [`Rebalance::fallout3_default`].
    fn default() -> Self {
        Self::fallout3_default()
    }
}

overlay_leaf!(SourceGame, WeaponClass, WeaponCode, [AttackMode; 2]);
overlay_struct!(ClassRule {
    code,
    modes,
    big_gun,
    two_handed,
    energy,
    ranges,
    min_strength,
    size,
    burst,
    critical_failure,
});
overlay_struct!(DamageRule { scale, spread, min });
overlay_struct!(ModeCosts {
    punch,
    kick,
    swing,
    thrust,
    throw,
    single,
    burst,
    continuous,
});
overlay_struct!(ApRule {
    base,
    source_divisor,
    extra,
    min,
    max,
});
overlay_struct!(RangeRule { units_per_hex, max });
overlay_struct!(BurstRule { seconds, min, max });
overlay_struct!(PerDamage {
    normal,
    laser,
    fire,
    plasma,
    electrical,
    emp,
    explosion,
});
overlay_struct!(ArmorRule {
    dr_scale,
    dr_per_dt,
    dt_scale,
    dt_per_dr,
    ac_base,
    ac_per_dr,
    ac_per_dt,
    emp_dr,
    size,
    dr_factors,
    dt_factors,
});
overlay_struct!(AmmoRule { default_pack });
overlay_struct!(AidRule {
    health_scale,
    radiation_scale,
    minutes_per_second,
    withdrawal_perk,
    withdrawal_onset,
});
overlay_struct!(CritterRule {
    hp_scale,
    hp_per_level,
    ap_divisor,
    ap_max,
    skill_scale,
    xp_base,
    xp_per_level,
    age,
});
overlay_struct!(Rebalance {
    game,
    default_class,
    unknown_caliber,
    accurate_spread_below,
    weight_scale,
    cost_scale,
    damage,
    ap,
    range,
    burst,
    armor,
    ammo,
    aid,
    critter,
    classes,
    calibers,
    skills,
});

/// The full Fallout 3 default as TOML (`Rebalance::fallout3_default`),
/// for `rebalance/fo3.toml`.
pub const FALLOUT3_TOML: &str = include_str!("../examples/rebalance-fo3.toml");
/// The full New Vegas default as TOML (`Rebalance::new_vegas_default`),
/// for `rebalance/fnv.toml`.
pub const NEW_VEGAS_TOML: &str = include_str!("../examples/rebalance-fnv.toml");

/// A calibre or table key: ASCII letters and digits only, lower case.
pub fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn rule(
    code: WeaponCode,
    modes: [AttackMode; 2],
    ranges: [i32; 2],
    min_strength: i32,
    size: i32,
) -> ClassRule {
    ClassRule {
        code,
        modes,
        big_gun: false,
        two_handed: false,
        energy: false,
        ranges,
        min_strength,
        size,
        burst: 0,
        critical_failure: 0,
    }
}

fn default_classes() -> BTreeMap<WeaponClass, ClassRule> {
    use AttackMode as M;
    use WeaponClass as C;
    use WeaponCode as W;
    let none = M::None;
    let mut map = BTreeMap::new();
    map.insert(C::Pistol, rule(W::Pistol, [M::Single, none], [20, 0], 3, 2));
    map.insert(
        C::Smg,
        ClassRule {
            burst: 10,
            two_handed: true,
            ..rule(W::Smg, [M::Single, M::Burst], [20, 15], 4, 3)
        },
    );
    map.insert(
        C::Rifle,
        ClassRule {
            two_handed: true,
            ..rule(W::Shotgun, [M::Single, none], [40, 0], 5, 5)
        },
    );
    map.insert(
        C::AssaultRifle,
        ClassRule {
            burst: 8,
            two_handed: true,
            ..rule(W::Smg, [M::Single, M::Burst], [35, 25], 5, 5)
        },
    );
    map.insert(
        C::Shotgun,
        ClassRule {
            two_handed: true,
            critical_failure: 2,
            ..rule(W::Shotgun, [M::Single, none], [12, 0], 4, 4)
        },
    );
    map.insert(
        C::Minigun,
        ClassRule {
            big_gun: true,
            two_handed: true,
            burst: 30,
            critical_failure: 2,
            ..rule(W::Minigun, [M::Burst, none], [30, 0], 7, 8)
        },
    );
    map.insert(
        C::Flamer,
        ClassRule {
            big_gun: true,
            two_handed: true,
            burst: 5,
            critical_failure: 4,
            ..rule(W::Minigun, [M::Burst, none], [5, 0], 6, 7)
        },
    );
    map.insert(
        C::Launcher,
        ClassRule {
            big_gun: true,
            two_handed: true,
            critical_failure: 4,
            ..rule(W::Launcher, [M::Single, none], [40, 0], 6, 8)
        },
    );
    map.insert(
        C::EnergyPistol,
        ClassRule {
            energy: true,
            ..rule(W::Pistol, [M::Single, none], [30, 0], 3, 2)
        },
    );
    map.insert(
        C::EnergyRifle,
        ClassRule {
            energy: true,
            two_handed: true,
            ..rule(W::LaserRifle, [M::Single, none], [45, 0], 6, 6)
        },
    );
    map.insert(
        C::EnergyHeavy,
        ClassRule {
            energy: true,
            big_gun: true,
            two_handed: true,
            burst: 20,
            ..rule(W::Minigun, [M::Burst, none], [30, 0], 7, 8)
        },
    );
    map.insert(
        C::Blade,
        rule(W::Knife, [M::Swing, M::Thrust], [1, 1], 2, 1),
    );
    map.insert(C::Blunt, rule(W::Club, [M::Swing, M::Thrust], [1, 1], 3, 3));
    map.insert(
        C::HeavyMelee,
        ClassRule {
            two_handed: true,
            ..rule(W::Hammer, [M::Swing, none], [1, 0], 6, 5)
        },
    );
    map.insert(
        C::Spear,
        ClassRule {
            two_handed: true,
            ..rule(W::Spear, [M::Thrust, M::Throw], [2, 15], 4, 4)
        },
    );
    map.insert(C::Unarmed, rule(W::Unarmed, [M::Punch, none], [1, 0], 1, 1));
    map.insert(C::Thrown, rule(W::Unarmed, [M::Throw, none], [15, 0], 2, 1));
    map.insert(
        C::Grenade,
        rule(W::Unarmed, [M::Throw, none], [15, 0], 3, 1),
    );
    map
}

/// The default calibre table, shared by both games: this crate's own
/// numbers from 101 up (module notes). Several keys may share a number.
pub fn default_calibers() -> BTreeMap<String, i32> {
    let rows: [(&[&str], i32); 27] = [
        (&["10mm"], 101),
        (&["556mm", "556", "556x45"], 102),
        (&["32", "32caliber", "32cal"], 103),
        (&["308", "308caliber"], 104),
        (&["5mm"], 105),
        (
            &[
                "12gauge",
                "12ga",
                "shotgunshell",
                "shotgunshells",
                "shell",
                "shells",
            ],
            106,
        ),
        (&["44", "44magnum", "44mag"], 107),
        (&["energycell", "smallenergycell", "sec"], 108),
        (&["microfusioncell", "mfc", "mfcell"], 109),
        (&["electronchargepack", "ecp"], 110),
        (&["flamerfuel", "fuel"], 111),
        (&["missile", "missiles", "rocket", "rockets"], 112),
        (&["mininuke", "mininukes"], 113),
        (&["bb", "bbs"], 114),
        (&["railwayspike", "railwayspikes"], 115),
        (&["alienpowercell", "alienpowermodule"], 116),
        (&["9mm"], 117),
        (&["357", "357magnum"], 118),
        (&["20gauge", "20ga"], 119),
        (&["45", "45auto", "45acp"], 120),
        (&["127mm", "127"], 121),
        (&["50mg", "50bmg", "50"], 122),
        (&["40mmgrenade", "40mm"], 123),
        (&["25mmgrenade", "25mm"], 124),
        (&["2mmec", "2mm"], 125),
        (&["4570", "4570govt", "4570gov"], 126),
        (&["22lr", "22"], 127),
    ];
    rows.iter()
        .flat_map(|(keys, caliber)| keys.iter().map(|key| (normalize_key(key), *caliber)))
        .collect()
}

fn default_skills(game: SourceGame) -> BTreeMap<SourceSkill, Vec<Skill>> {
    use SourceSkill as S;
    let mut map = vec![
        (S::Barter, Skill::Barter),
        (S::BigGuns, Skill::BigGuns),
        (S::EnergyWeapons, Skill::EnergyWeapons),
        (S::Explosives, Skill::Traps),
        (S::Explosives, Skill::Throwing),
        (S::Lockpick, Skill::Lockpick),
        (S::Medicine, Skill::FirstAid),
        (S::Medicine, Skill::Doctor),
        (S::MeleeWeapons, Skill::MeleeWeapons),
        (S::Repair, Skill::Repair),
        (S::Science, Skill::Science),
        (S::SmallGuns, Skill::SmallGuns),
        (S::Sneak, Skill::Sneak),
        (S::Sneak, Skill::Steal),
        (S::Speech, Skill::Speech),
        (S::Survival, Skill::Outdoorsman),
        (S::Unarmed, Skill::Unarmed),
    ];
    if game == SourceGame::NewVegas {
        // One Guns skill covers both of Fallout 2's.
        map.push((S::Guns, Skill::SmallGuns));
        map.push((S::Guns, Skill::BigGuns));
    }
    let mut out: BTreeMap<SourceSkill, Vec<Skill>> = BTreeMap::new();
    for (from, to) in map {
        out.entry(from).or_default().push(to);
    }
    out
}

impl Rebalance {
    /// Fallout 3: damage × 1.0 spread 0.6 to 1.4; VATS AP / 4; 64 units
    /// to a hex; armour DR × 2, DT 0.15 × DR, AC = DR; HP × 0.5 + level;
    /// costs and weights × 1.
    pub fn fallout3_default() -> Self {
        Self {
            game: SourceGame::Fallout3,
            default_class: WeaponClass::Pistol,
            unknown_caliber: 199,
            accurate_spread_below: None,
            weight_scale: 1.0,
            cost_scale: 1.0,
            damage: DamageRule::default(),
            ap: ApRule::default(),
            range: RangeRule::default(),
            burst: BurstRule::default(),
            armor: ArmorRule::default(),
            ammo: AmmoRule::default(),
            aid: AidRule::default(),
            critter: CritterRule::default(),
            classes: default_classes(),
            calibers: default_calibers(),
            skills: default_skills(SourceGame::Fallout3),
        }
    }

    /// New Vegas: damage × 0.6; armour DT × 0.3, DR 2.5 × DT plus DR, AC
    /// 1.2 × DT plus half the DR; HP × 0.4 + level; Guns sets Small and
    /// Big Guns; otherwise as Fallout 3.
    pub fn new_vegas_default() -> Self {
        let base = Self::fallout3_default();
        Self {
            game: SourceGame::NewVegas,
            damage: DamageRule {
                scale: 0.6,
                ..base.damage
            },
            armor: ArmorRule {
                dr_scale: 1.0,
                dr_per_dt: 2.5,
                dt_scale: 0.3,
                dt_per_dr: 0.0,
                ac_base: 0.0,
                ac_per_dr: 0.5,
                ac_per_dt: 1.2,
                ..base.armor
            },
            critter: CritterRule {
                hp_scale: 0.4,
                ..base.critter
            },
            skills: default_skills(SourceGame::NewVegas),
            ..base
        }
    }

    /// The default for `game`.
    pub fn for_game(game: SourceGame) -> Self {
        match game {
            SourceGame::Fallout3 => Self::fallout3_default(),
            SourceGame::NewVegas => Self::new_vegas_default(),
        }
    }

    /// The calibre number for `key`; both it and the table's keys are
    /// compared through [`normalize_key`].
    pub fn caliber(&self, key: &str) -> Option<i32> {
        let key = normalize_key(key);
        self.calibers
            .get(&key)
            .or_else(|| {
                self.calibers
                    .iter()
                    .find(|(known, _)| normalize_key(known) == key)
                    .map(|(_, caliber)| caliber)
            })
            .copied()
    }

    /// Overlay a document over `self`: each key it gives replaces that
    /// value (nested tables key by key; a class or calibre it names that
    /// the table lacks is added whole); every key it leaves out keeps its
    /// value; an unknown key is an error. With the `toml` crate:
    ///
    /// ```text
    /// let mut table = Rebalance::for_game(game);
    /// table.overlay(toml::Deserializer::parse(&text)?)?;
    /// ```
    pub fn overlay<'de, D: serde::Deserializer<'de>>(
        &mut self,
        deserializer: D,
    ) -> Result<(), D::Error> {
        Overlay::overlay(self, deserializer)
    }

    fn caliber_or_unknown(&self, key: Option<&str>) -> i32 {
        key.and_then(|key| self.caliber(key))
            .unwrap_or(self.unknown_caliber)
    }

    fn class(&self, class: Option<WeaponClass>) -> ClassRule {
        let class = class.unwrap_or(self.default_class);
        self.classes
            .get(&class)
            .or_else(|| self.classes.get(&self.default_class))
            .copied()
            .unwrap_or_else(|| {
                rule(
                    WeaponCode::Pistol,
                    [AttackMode::Single, AttackMode::None],
                    [20, 0],
                    3,
                    2,
                )
            })
    }

    fn stats(&self, size: i32, weight: f32, value: f32) -> ItemStats {
        ItemStats {
            size: size.max(0),
            weight: round(weight * self.weight_scale).max(0),
            cost: round(value * self.cost_scale).max(0),
            inventory_art: None,
            sound: 0,
        }
    }
}

fn round(value: f32) -> i32 {
    if value.is_finite() {
        value.round().clamp(i32::MIN as f32, i32::MAX as f32) as i32
    } else {
        0
    }
}

fn damage_type(kind: SourceDamage, energy: bool) -> DamageType {
    match kind {
        SourceDamage::Ballistic if energy => DamageType::Laser,
        SourceDamage::Ballistic => DamageType::Normal,
        SourceDamage::Laser => DamageType::Laser,
        SourceDamage::Plasma => DamageType::Plasma,
        SourceDamage::Fire => DamageType::Fire,
        SourceDamage::Electric => DamageType::Electrical,
        SourceDamage::Pulse => DamageType::Emp,
        SourceDamage::Explosive => DamageType::Explosion,
    }
}

/// A [`WeaponSpec`] for `source` under `rebalance` (see the rule types for
/// each formula). The result passes [`crate::items::weapon`]'s checks.
/// Left for the caller: the inventory art, the default ammunition PID
/// (`ammo`), a projectile PID, the sound bytes.
pub fn weapon_spec(source: &SourceWeapon, rebalance: &Rebalance) -> WeaponSpec {
    let class = rebalance.class(source.class);
    let mut modes = class.modes;
    if source.automatic && modes[0] == AttackMode::Single && modes[1] == AttackMode::None {
        modes[1] = AttackMode::Burst;
    }
    let ranged = modes.iter().any(|mode| mode.is_ranged_fire());
    let damage_type = damage_type(source.damage_kind, class.energy && ranged);
    let shot = source.damage.max(0.0) * source.projectiles.max(1) as f32 * rebalance.damage.scale;
    let low = round(shot * rebalance.damage.spread[0])
        .max(rebalance.damage.min)
        .max(0);
    let high = round(shot * rebalance.damage.spread[1]).max(low);
    let ap = |mode: AttackMode| -> i32 {
        let cost = if rebalance.ap.source_divisor > 0.0 && source.ap > 0.0 {
            round(source.ap / rebalance.ap.source_divisor) + rebalance.ap.extra.get(mode)
        } else {
            rebalance.ap.base.get(mode)
        };
        cost.clamp(rebalance.ap.min.max(1), rebalance.ap.max.clamp(1, 9))
    };
    let range = |slot: usize, mode: AttackMode| -> i32 {
        let default = class.ranges[slot].max(1);
        if mode.is_ranged_fire() && source.range > 0.0 && rebalance.range.units_per_hex > 0.0 {
            round(source.range / rebalance.range.units_per_hex).clamp(1, rebalance.range.max.max(1))
        } else {
            default
        }
    };
    let mut ap_cost = [0; 2];
    let mut ranges = [0; 2];
    for (slot, mode) in modes.iter().enumerate() {
        if *mode != AttackMode::None {
            ap_cost[slot] = ap(*mode);
            ranges[slot] = range(slot, *mode);
        }
    }
    let bursts = modes.contains(&AttackMode::Burst);
    let mut capacity = if ranged {
        i32::try_from(source.clip)
            .unwrap_or(i32::MAX)
            .clamp(1, 9_999)
    } else {
        0
    };
    let mut burst = 0;
    if bursts {
        burst = if source.fire_rate > 0.0 {
            round(source.fire_rate * rebalance.burst.seconds)
                .clamp(rebalance.burst.min, rebalance.burst.max)
        } else {
            class.burst
        }
        .max(2);
        capacity = capacity.max(2);
        burst = burst.min(capacity);
    }
    let perk = rebalance.accurate_spread_below.and_then(|limit| {
        (ranged && source.min_spread > 0.0 && source.min_spread < limit)
            .then_some(weapon_perk::ACCURATE)
    });
    WeaponSpec {
        stats: rebalance.stats(class.size, source.weight, source.value as f32),
        code: class.code,
        damage: [low, high],
        damage_type,
        modes,
        ap_cost,
        ranges,
        min_strength: if source.strength > 0 {
            source.strength.clamp(0, 10)
        } else {
            class.min_strength.clamp(0, 10)
        },
        projectile: None,
        caliber: if capacity > 0 {
            rebalance.caliber_or_unknown(source.caliber.as_deref())
        } else {
            0
        },
        ammo: None,
        capacity,
        burst,
        perk,
        critical_failure: class.critical_failure.clamp(0, 6),
        big_gun: class.big_gun,
        two_handed: class.two_handed || source.two_handed,
        sound: 0,
    }
}

/// An [`ArmorSpec`] for `source` ([`ArmorRule`]). Left for the caller: the
/// inventory art and the male and female figure art.
pub fn armor_spec(source: &SourceArmor, rebalance: &Rebalance) -> ArmorSpec {
    let rule = &rebalance.armor;
    let (dr, dt) = (source.dr.max(0.0), source.dt.max(0.0));
    let normal_dr = (dr * rule.dr_scale + dt * rule.dr_per_dt).clamp(0.0, 90.0);
    let normal_dt = (dt * rule.dt_scale + dr * rule.dt_per_dr).clamp(0.0, 100.0);
    let mut resistances = [0; 7];
    let mut thresholds = [0; 7];
    for kind in DamageType::ALL {
        let index = kind.index();
        resistances[index] = round(normal_dr * rule.dr_factors.get(kind)).clamp(0, 90);
        thresholds[index] = round(normal_dt * rule.dt_factors.get(kind)).clamp(0, 100);
    }
    resistances[DamageType::Emp.index()] = rule.emp_dr.max(0);
    thresholds[DamageType::Emp.index()] = 0;
    ArmorSpec {
        stats: rebalance.stats(rule.size, source.weight, source.value as f32),
        armor_class: round(rule.ac_base + dr * rule.ac_per_dr + dt * rule.ac_per_dt).clamp(0, 90),
        resistances,
        thresholds,
        perk: source.power_armor.then_some(armor_perk::POWERED_ARMOR),
        male_art: None,
        female_art: None,
    }
}

/// The fraction `n / d` (both 1 to 8) nearest `value`, smaller terms
/// first on a tie.
fn fraction(value: f32) -> (i32, i32) {
    let value = if value.is_finite() && value > 0.0 {
        value
    } else {
        1.0
    };
    let mut best = (1, 1);
    let mut error = f32::INFINITY;
    for d in 1..=8 {
        for n in 1..=8 {
            let e = (n as f32 / d as f32 - value).abs();
            if e < error - 1e-6 {
                (best, error) = ((n, d), e);
            }
        }
    }
    best
}

/// An [`AmmoSpec`] for `source`: pack size from the source or
/// [`AmmoRule::default_pack`], cost and weight per pack, calibre from the
/// table, the damage multiplier as the nearest fraction of terms 1 to 8.
/// Left for the caller: the inventory art.
pub fn ammo_spec(source: &SourceAmmo, rebalance: &Rebalance) -> AmmoSpec {
    let pack = if source.pack > 0 {
        i32::try_from(source.pack).unwrap_or(i32::MAX)
    } else {
        rebalance.ammo.default_pack.max(1)
    };
    let (multiplier, divisor) = fraction(source.damage_mult);
    AmmoSpec {
        stats: rebalance.stats(
            1,
            source.weight.max(0.0) * pack as f32,
            source.value.max(0.0) * pack as f32,
        ),
        caliber: rebalance.caliber_or_unknown(Some(&source.caliber)),
        quantity: pack,
        ac_modifier: source.ac_modifier,
        dr_modifier: source.dr_modifier,
        multiplier,
        divisor,
    }
}

/// A [`DrugSpec`] for `source` ([`AidRule`]). Slots in order: hit points
/// (stat 35), radiation (37, removing is negative; it acts on the player
/// only), then the effects, at most three slots in all (later effects are
/// dropped). Effects apply at once; a timed effect wears off through the
/// first delayed stage, whose delay is the first timed effect's duration
/// (the format has one delay per stage for all three slots).
pub fn aid_spec(source: &SourceAid, rebalance: &Rebalance) -> DrugSpec {
    let rule = &rebalance.aid;
    let mut spec = DrugSpec::instant(drug_stat::CURRENT_HP, 0);
    spec.slots = [DrugStat::Unused; 3];
    spec.immediate = [0; 3];
    spec.stats = rebalance.stats(1, source.weight, source.value as f32);
    let mut slot = 0;
    let mut push = |stat: u8, now: i32, later: i32, spec: &mut DrugSpec| {
        if slot < 3 {
            spec.slots[slot] = DrugStat::Stat(stat);
            spec.immediate[slot] = now;
            spec.first[slot] = later;
            slot += 1;
        }
    };
    if source.health != 0 {
        let health = round(source.health as f32 * rule.health_scale);
        push(drug_stat::CURRENT_HP, health, 0, &mut spec);
    }
    if source.radiation != 0 {
        let radiation = -round(source.radiation as f32 * rule.radiation_scale);
        push(drug_stat::RADIATION, radiation, 0, &mut spec);
    }
    let mut delay = None;
    for effect in &source.effects {
        let timed = effect.seconds > 0.0;
        if timed && delay.is_none() {
            delay = Some(round(effect.seconds * rule.minutes_per_second).max(1));
        }
        let later = if timed { -effect.amount } else { 0 };
        push(effect.stat.index(), effect.amount, later, &mut spec);
    }
    spec.first_delay = delay.unwrap_or(0);
    if delay.is_none() {
        spec.first = [0; 3];
    }
    spec.addiction_chance = round(source.addiction_chance * 100.0).clamp(0, 100);
    if spec.addiction_chance > 0 {
        spec.withdrawal_perk = Some(source.withdrawal.unwrap_or(rule.withdrawal_perk));
        spec.withdrawal_onset = rule.withdrawal_onset.max(0);
    }
    spec
}

/// A [`FighterSpec`] for `source` ([`CritterRule`]) with `ai_packet` and
/// `team`; `worn` is the converted armour the actor wears, whose AC, DR
/// and DT go into the prototype (module notes).
pub fn fighter_spec(
    source: &SourceActor,
    worn: Option<&ArmorSpec>,
    rebalance: &Rebalance,
    ai_packet: i32,
    team: i32,
) -> FighterSpec {
    let rule = &rebalance.critter;
    let special = source.special.map(|value| value.clamp(1, 10));
    let hit_points = round(
        source.health.max(0) as f32 * rule.hp_scale
            + source.level.max(0) as f32 * rule.hp_per_level,
    )
    .max(1);
    let mut critter = CritterSpec::human(hit_points, ai_packet.max(0), team.max(0));
    critter.special = special;
    critter.female = source.female;
    critter.age = rule.age.clamp(16, 35);
    critter.experience = (rule.xp_base + source.level.max(0) * rule.xp_per_level).max(0);
    let mut spec = FighterSpec::new(critter);
    if rule.ap_divisor > 0.0 && source.action_points > 0 {
        spec.action_points =
            Some(round(source.action_points as f32 / rule.ap_divisor).clamp(1, rule.ap_max.max(1)));
    }
    for (from, value) in &source.skills {
        let value = round(*value as f32 * rule.skill_scale).clamp(0, 200);
        for skill in rebalance.skills.get(from).into_iter().flatten() {
            let slot = &mut spec.skills[skill.index()];
            *slot = Some(slot.map_or(value, |kept| kept.max(value)));
        }
    }
    if let Some(armor) = worn {
        spec.armor_class = Some(special[5] + armor.armor_class);
        spec.resistances = armor.resistances;
        spec.thresholds = armor.thresholds;
    }
    spec
}
