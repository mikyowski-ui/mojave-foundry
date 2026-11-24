//! Item prototypes with typed specifications: weapons, armour,
//! ammunition, drugs, keys and powered miscellaneous items.
//!
//! Every constructor validates its spec, then fills `vault_formats::pro`'s
//! `ItemData` variant; `Prototype::write` lays it out and
//! `Prototype::parse_for` reads it back equal (the payloads are SPEC-003
//! §2.3, the same under Fallout 2, SPEC-016 §1). Each field's doc says
//! where the format keeps it and what the engine does with it, per the
//! approved specifications. Names and descriptions go through
//! [`crate::text::describe`].
//!
//! Shared item words (`Item`, SPEC-003 §2.3; SPEC-011 §2.4, §3.14, §3.16):
//! `size` counts against a container's capacity, `weight` against carry
//! weight, `cost` is the base value barter starts from; the inventory art
//! is an `art/inven` FID; the stored byte is the item's sound letter, used
//! for containers (SPEC-010 §2.5).

use crate::EmitError;
use crate::animation::{AttackMode, WeaponCode};
use crate::proto::{ART_ITEMS, ItemStats, NO_PID, ProtoCommon, check_common, finish, item};
use vault_formats::pro::{Armor, Category, Drug, ExtendedFlags, ItemData, PrototypeData, Weapon};
use vault_formats::{Fid, Prototype};

/// Damage types 0 to 6, in the order every per-type array uses (SPEC-003
/// §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(i32)]
pub enum DamageType {
    #[default]
    Normal = 0,
    Laser = 1,
    Fire = 2,
    Plasma = 3,
    Electrical = 4,
    Emp = 5,
    Explosion = 6,
}

impl DamageType {
    pub const ALL: [DamageType; 7] = [
        Self::Normal,
        Self::Laser,
        Self::Fire,
        Self::Plasma,
        Self::Electrical,
        Self::Emp,
        Self::Explosion,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Laser, plasma and electrical: ranged fire with these uses Energy
    /// Weapons (SPEC-007 §3.5).
    pub fn is_energy(self) -> bool {
        matches!(self, Self::Laser | Self::Plasma | Self::Electrical)
    }
}

/// The 18 skills, in the prototype's skill-array order (SPEC-003 §2.4).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum Skill {
    SmallGuns = 0,
    BigGuns = 1,
    EnergyWeapons = 2,
    Unarmed = 3,
    MeleeWeapons = 4,
    Throwing = 5,
    FirstAid = 6,
    Doctor = 7,
    Sneak = 8,
    Lockpick = 9,
    Steal = 10,
    Traps = 11,
    Science = 12,
    Repair = 13,
    Speech = 14,
    Barter = 15,
    Gambling = 16,
    Outdoorsman = 17,
}

impl Skill {
    pub const ALL: [Skill; 18] = [
        Self::SmallGuns,
        Self::BigGuns,
        Self::EnergyWeapons,
        Self::Unarmed,
        Self::MeleeWeapons,
        Self::Throwing,
        Self::FirstAid,
        Self::Doctor,
        Self::Sneak,
        Self::Lockpick,
        Self::Steal,
        Self::Traps,
        Self::Science,
        Self::Repair,
        Self::Speech,
        Self::Barter,
        Self::Gambling,
        Self::Outdoorsman,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The untrained base percentage from effective SPECIAL `[ST, PE, EN,
    /// CH, IN, AG, LK]` (SPEC-006 §3.3).
    pub fn base(self, special: [i32; 7]) -> i32 {
        let [st, pe, en, ch, int, ag, lk] = special;
        match self {
            Self::SmallGuns => 35 + ag,
            Self::BigGuns | Self::EnergyWeapons => 10 + ag,
            Self::Unarmed => 65 + (ag + st) / 2,
            Self::MeleeWeapons => 55 + (ag + st) / 2,
            Self::Throwing => 40 + ag,
            Self::FirstAid => 30 + (pe + int) / 2,
            Self::Doctor => 15 + (pe + int) / 2,
            Self::Sneak => 25 + ag,
            Self::Lockpick | Self::Traps => 20 + (pe + ag) / 2,
            Self::Steal => 20 + ag,
            Self::Science => 25 + 2 * int,
            Self::Repair => 20 + int,
            Self::Speech => 25 + 2 * ch,
            Self::Barter => 20 + 2 * ch,
            Self::Gambling => 20 + 3 * lk,
            Self::Outdoorsman => 5 + (en + int) / 2,
        }
    }
}

/// The skill an attack mode uses, as the engine derives it (SPEC-007
/// §3.5): punch and kick Unarmed; swing and thrust Melee; throw Throwing;
/// ranged fire Energy Weapons for laser, plasma and electrical damage,
/// else Big Guns with the big-gun flag, else Small Guns. Mode none has no
/// skill. The format stores no skill: this is how it is implied.
pub fn implied_skill(mode: AttackMode, damage: DamageType, big_gun: bool) -> Option<Skill> {
    Some(match mode {
        AttackMode::None => return None,
        AttackMode::Punch | AttackMode::Kick => Skill::Unarmed,
        AttackMode::Swing | AttackMode::Thrust => Skill::MeleeWeapons,
        AttackMode::Throw => Skill::Throwing,
        _ if damage.is_energy() => Skill::EnergyWeapons,
        _ if big_gun => Skill::BigGuns,
        _ => Skill::SmallGuns,
    })
}

/// Weapon perk selectors (SPEC-011 §3.22, read by SPEC-007's attack
/// rules).
pub mod weapon_perk {
    pub const LONG_RANGE: i32 = 58;
    pub const ACCURATE: i32 = 59;
    pub const PENETRATE: i32 = 60;
    pub const KNOCKBACK: i32 = 61;
}

/// Armour perk selectors (SPEC-011 §3.22); applied to the player only.
pub mod armor_perk {
    pub const POWERED_ARMOR: i32 = 62;
    pub const COMBAT_ARMOR: i32 = 63;
}

/// Withdrawal perk selectors 53 to 57 (SPEC-011 §3.22), by their effect.
pub mod withdrawal_perk {
    /// Description only, no stat change.
    pub const DESCRIPTION_ONLY: i32 = 53;
    /// ST -2, EN -2, AG -3.
    pub const STRENGTH_ENDURANCE_AGILITY: i32 = 54;
    /// IN -3, AG -2.
    pub const INTELLIGENCE_AGILITY: i32 = 55;
    /// IN -2.
    pub const INTELLIGENCE: i32 = 56;
    /// Radiation resistance -20.
    pub const RADIATION_RESISTANCE: i32 = 57;
}

/// Positions of the drug stat selectors beyond the 35 stats (SPEC-011
/// §2.1).
pub mod drug_stat {
    /// Current hit points (instance state).
    pub const CURRENT_HP: u8 = 35;
    /// Current poison level; acts on the player only.
    pub const POISON: u8 = 36;
    /// Current radiation level; acts on the player only.
    pub const RADIATION: u8 = 37;
}

fn bad<T>(message: String) -> Result<T, EmitError> {
    Err(EmitError::Prototype(message))
}

fn check_perk(perk: Option<i32>, what: &str) -> Result<i32, EmitError> {
    match perk {
        None => Ok(-1),
        Some(perk @ 0..=63) => Ok(perk),
        Some(perk) => bad(format!("{what} perk {perk} is outside 0 to 63")),
    }
}

fn check_item_pid(pid: Option<u32>, what: &str) -> Result<i32, EmitError> {
    match pid {
        None => Ok(NO_PID),
        Some(pid) if pid >> 24 == Category::Item as u32 && pid & 0x00ff_ffff != 0 => Ok(pid as i32),
        Some(pid) => bad(format!("{what} {pid:#010x} is not a file-backed item PID")),
    }
}

fn finish_item(
    common: &ProtoCommon,
    stats: ItemStats,
    extended: ExtendedFlags,
    data: ItemData,
) -> Result<Prototype, EmitError> {
    check_common(common, Category::Item, ART_ITEMS)?;
    if stats.size < 0 || stats.weight < 0 || stats.cost < 0 {
        return bad(format!(
            "size {}, weight {} and cost {} must not be negative",
            stats.size, stats.weight, stats.cost
        ));
    }
    let item = item(common, stats, data)?;
    Ok(finish(
        common,
        Category::Item,
        extended,
        PrototypeData::Item(item),
    ))
}

/// A weapon (item subtype 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponSpec {
    pub stats: ItemStats,
    /// `Weapon::animation`: the weapon code a critter's art takes while it
    /// holds the weapon, so its stand, walk and attack files are the
    /// lettered ones (SPEC-007 §2.2, SPEC-005 §3.8).
    pub code: WeaponCode,
    /// `Weapon::damage`, minimum then maximum. A ranged or thrown hit
    /// draws in this range; melee and unarmed add the attacker's melee
    /// damage to the maximum (SPEC-007 §3.6). The writer keeps reader
    /// order (SPEC-003 §3.1 safe rule).
    pub damage: [i32; 2],
    /// `Weapon::damage_type`: selects the defender's DT and DR, the
    /// death animation (SPEC-007 §3.8), and with ranged fire the skill.
    pub damage_type: DamageType,
    /// The extended flags' low and next nibbles (SPEC-003 §2.6): primary
    /// and secondary attack modes. The primary must not be none.
    pub modes: [AttackMode; 2],
    /// `Weapon::ap_cost`, per mode: the attack's AP before perks and
    /// aiming (SPEC-007 §3.4). 1 to 9 for a mode that exists: the
    /// interface bar draws costs 0 to 9 only (SPEC-014 §3.23).
    pub ap_cost: [i32; 2],
    /// `Weapon::ranges`, per mode, in hexes (SPEC-007 §3.4; a throw is
    /// also capped at 3 × Strength). At least 1 for a mode that exists.
    pub ranges: [i32; 2],
    /// `Weapon::strength`: each point above the attacker's Strength costs
    /// 20 hit chance; never an equip test (SPEC-011 §3.11). 0 to 10.
    pub min_strength: i32,
    /// `Weapon::projectile_pid`: a miscellaneous PID whose art flies to
    /// the target (SPEC-007 §3.9, §3.10.4). `None` (-1) for bullets: no
    /// projectile is shown and the target reacts on the action frame.
    pub projectile: Option<u32>,
    /// `Weapon::caliber`: reload accepts ammunition of an equal calibre;
    /// the number has no other meaning (SPEC-011 §3.15). Above 0 when the
    /// weapon has a magazine.
    pub caliber: i32,
    /// `Weapon::ammo_pid`: the default ammunition, which a weapon on a
    /// base map is loaded with (SPEC-003 §3.3).
    pub ammo: Option<u32>,
    /// `Weapon::capacity`: the magazine. A weapon on a base map loads with
    /// exactly this many rounds (SPEC-003 §3.3). Ranged fire needs one.
    pub capacity: i32,
    /// `Weapon::burst`: rounds per burst (SPEC-007 §3.7).
    pub burst: i32,
    /// `Weapon::perk`: a [`weapon_perk`] selector, or `None` (-1).
    pub perk: Option<i32>,
    /// `Weapon::critical_failure`: the critical-failure table 0 to 6
    /// (SPEC-007 §2.4).
    pub critical_failure: i32,
    /// Extended flag `0x100`: ranged fire uses Big Guns unless the damage
    /// is energy (SPEC-007 §3.5).
    pub big_gun: bool,
    /// Extended flag `0x200`: refused with a crippled arm, and the One
    /// Hander trait's penalty (SPEC-011 §3.11). Never an equip test.
    pub two_handed: bool,
    /// `Weapon::sound`: the weapon sound byte, the letter in its effect
    /// names (SPEC-010 §2.5); 0, or an ASCII letter or digit.
    pub sound: u8,
}

impl WeaponSpec {
    /// A one-mode weapon with no magazine, no perk, failure table 0, no
    /// projectile, no flags, sound 0: set the rest by field.
    pub fn new(
        code: WeaponCode,
        damage: [i32; 2],
        damage_type: DamageType,
        mode: AttackMode,
        ap_cost: i32,
        range: i32,
    ) -> Self {
        Self {
            stats: ItemStats::default(),
            code,
            damage,
            damage_type,
            modes: [mode, AttackMode::None],
            ap_cost: [ap_cost, 0],
            ranges: [range, 0],
            min_strength: 0,
            projectile: None,
            caliber: 0,
            ammo: None,
            capacity: 0,
            burst: 0,
            perk: None,
            critical_failure: 0,
            big_gun: false,
            two_handed: false,
            sound: 0,
        }
    }

    /// The skill mode `slot` (0 primary, 1 secondary) uses
    /// ([`implied_skill`]).
    pub fn skill(&self, slot: usize) -> Option<Skill> {
        implied_skill(*self.modes.get(slot)?, self.damage_type, self.big_gun)
    }

    /// The extended flag word: modes, big gun, two-handed.
    pub fn extended(&self) -> ExtendedFlags {
        let mut bits = u32::from(self.modes[0].code()) | (u32::from(self.modes[1].code()) << 4);
        if self.big_gun {
            bits |= ExtendedFlags::BIG_GUN.0;
        }
        if self.two_handed {
            bits |= ExtendedFlags::TWO_HANDED.0;
        }
        ExtendedFlags(bits)
    }

    fn check(&self) -> Result<(), EmitError> {
        let [low, high] = self.damage;
        if low < 0 || high < low {
            return bad(format!("damage {low} to {high} is not 0 <= min <= max"));
        }
        if self.modes[0] == AttackMode::None {
            return bad("a weapon's primary attack mode must not be none".into());
        }
        for (slot, mode) in self.modes.iter().enumerate() {
            if *mode == AttackMode::None {
                continue;
            }
            if !(1..=9).contains(&self.ap_cost[slot]) {
                return bad(format!(
                    "mode {slot} AP cost {} is outside 1 to 9",
                    self.ap_cost[slot]
                ));
            }
            if self.ranges[slot] < 1 {
                return bad(format!(
                    "mode {slot} range {} is below 1",
                    self.ranges[slot]
                ));
            }
            if let Some(anim) = mode.animation()
                && anim.needs_weapon()
                && self.code == WeaponCode::Unarmed
            {
                return bad(format!(
                    "{mode:?} plays a weapon action, which needs a weapon code other than 0"
                ));
            }
            if mode.is_ranged_fire() && self.capacity < 1 {
                return bad(format!(
                    "{mode:?} fire needs a magazine (capacity 1 or more)"
                ));
            }
            if *mode == AttackMode::Burst && self.burst < 2 {
                return bad(format!("a burst of {} rounds; at least 2", self.burst));
            }
        }
        if self.burst < 0 {
            return bad(format!("burst {} is negative", self.burst));
        }
        if !(0..=10).contains(&self.min_strength) {
            return bad(format!(
                "minimum strength {} is outside 0 to 10",
                self.min_strength
            ));
        }
        if self.capacity < 0 {
            return bad(format!("capacity {} is negative", self.capacity));
        }
        if self.capacity > 0 && self.caliber < 1 {
            return bad("a weapon with a magazine needs a calibre above 0".into());
        }
        if self.capacity == 0 && self.ammo.is_some() {
            return bad("a weapon without a magazine takes no ammunition".into());
        }
        if !(0..=6).contains(&self.critical_failure) {
            return bad(format!(
                "critical-failure table {} is outside 0 to 6",
                self.critical_failure
            ));
        }
        if let Some(pid) = self.projectile
            && (pid >> 24 != Category::Misc as u32 || pid & 0x00ff_ffff == 0)
        {
            return bad(format!("projectile {pid:#010x} is not a miscellaneous PID"));
        }
        if self.sound != 0 && !self.sound.is_ascii_alphanumeric() {
            return bad(format!(
                "weapon sound byte {:#04x} is not 0, a letter or a digit",
                self.sound
            ));
        }
        Ok(())
    }
}

/// A weapon (item subtype 3): extended flags from [`WeaponSpec::extended`]
/// (weapons are picked up by subtype). Refused: what
/// [`WeaponSpec`]'s field notes rule out.
pub fn weapon(common: ProtoCommon, spec: &WeaponSpec) -> Result<Prototype, EmitError> {
    spec.check()?;
    let data = ItemData::Weapon(Weapon {
        animation: i32::from(spec.code.code()),
        damage: spec.damage,
        damage_type: spec.damage_type as i32,
        ranges: spec.ranges,
        projectile_pid: spec.projectile.map_or(NO_PID, |pid| pid as i32),
        strength: spec.min_strength,
        ap_cost: spec.ap_cost,
        critical_failure: spec.critical_failure,
        perk: check_perk(spec.perk, "weapon")?,
        burst: spec.burst,
        caliber: spec.caliber,
        ammo_pid: check_item_pid(spec.ammo, "default ammunition")?,
        capacity: spec.capacity,
        sound: spec.sound,
    });
    finish_item(&common, spec.stats, spec.extended(), data)
}

/// Armour (item subtype 0). Worn armour changes the wearer's figure and,
/// **for the player only**, adds its AC, DR, DT and perk to the bonus
/// stats; a non-player critter's protection comes from its prototype
/// alone (SPEC-011 §3.10, SPEC-005 §3.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmorSpec {
    pub stats: ItemStats,
    /// `Armor::armor_class`, 0 or more.
    pub armor_class: i32,
    /// `Armor::resistances`: DR percent by damage type. The effective
    /// stat is clamped to 0-90 (EMP 0-100); the raw value is stored.
    pub resistances: [i32; 7],
    /// `Armor::thresholds`: DT by damage type, subtracted per round
    /// before DR (SPEC-007 §3.6).
    pub thresholds: [i32; 7],
    /// `Armor::perk`: an [`armor_perk`] selector or `None` (-1).
    pub perk: Option<i32>,
    /// `Armor::male_art` / `female_art`: the critter art (category 1,
    /// animation 0) the wearer's figure takes; the FID's row is the base
    /// used. `None` stores -1, which the engine replaces by the default
    /// figure or row 1 depending on the path (SPEC-005 §3.8).
    pub male_art: Option<Fid>,
    pub female_art: Option<Fid>,
}

fn check_figure(art: Option<Fid>, what: &str) -> Result<i32, EmitError> {
    match art {
        None => Ok(-1),
        Some(fid) if fid.category() == 1 && fid.animation() == 0 && fid.weapon() == 0 => {
            Ok(fid.0 as i32)
        }
        Some(fid) => bad(format!(
            "{what} art {:#010x} is not critter art for animation 0 and weapon code 0",
            fid.0
        )),
    }
}

/// Armour (item subtype 0), extended flags 0.
pub fn armor(common: ProtoCommon, spec: &ArmorSpec) -> Result<Prototype, EmitError> {
    if spec.armor_class < 0
        || spec.resistances.iter().any(|v| *v < 0)
        || spec.thresholds.iter().any(|v| *v < 0)
    {
        return bad("armour class, resistances and thresholds must not be negative".into());
    }
    let data = ItemData::Armor(Armor {
        armor_class: spec.armor_class,
        resistances: spec.resistances,
        thresholds: spec.thresholds,
        perk: check_perk(spec.perk, "armour")?,
        male_art: check_figure(spec.male_art, "male")?,
        female_art: check_figure(spec.female_art, "female")?,
    });
    finish_item(&common, spec.stats, ExtendedFlags(0), data)
}

/// Ammunition (item subtype 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmoSpec {
    pub stats: ItemStats,
    /// `caliber`: matches the weapons it loads (SPEC-011 §3.15); above 0.
    pub caliber: i32,
    /// `quantity`: rounds per pack, K (SPEC-011 §2.4, §3.13); the weight,
    /// value and stacking rules divide by it, so at least 1.
    pub quantity: i32,
    /// `armor_modifier`, `resistance_modifier`, `multiplier`, `divisor`:
    /// stored, and not used by the engine's damage formula (SPEC-007 §3.6,
    /// "Ammunition multiplier, divisor, and DR modifier are not used
    /// here"). Kept for display and scripts; multiplier and divisor at
    /// least 1.
    pub ac_modifier: i32,
    pub dr_modifier: i32,
    pub multiplier: i32,
    pub divisor: i32,
}

/// Ammunition (item subtype 4), extended flags 0.
pub fn ammo(common: ProtoCommon, spec: &AmmoSpec) -> Result<Prototype, EmitError> {
    if spec.caliber < 1 || spec.quantity < 1 || spec.multiplier < 1 || spec.divisor < 1 {
        return bad(format!(
            "calibre {}, pack size {}, multiplier {} and divisor {} must each be at least 1",
            spec.caliber, spec.quantity, spec.multiplier, spec.divisor
        ));
    }
    let data = ItemData::Ammunition {
        caliber: spec.caliber,
        quantity: spec.quantity,
        armor_modifier: spec.ac_modifier,
        resistance_modifier: spec.dr_modifier,
        multiplier: spec.multiplier,
        divisor: spec.divisor,
    };
    finish_item(&common, spec.stats, ExtendedFlags(0), data)
}

/// A drug stat selector (SPEC-011 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrugStat {
    /// -1: the slot is skipped.
    #[default]
    Unused,
    /// -2, slot 0 only: the next used slot's amount is drawn from the
    /// range (slot 0's amount, slot 1's amount) (SPEC-011 §3.2).
    RandomRange,
    /// A stat 0 to 34 (bonus array), or [`drug_stat`] 35 to 37.
    Stat(u8),
}

impl DrugStat {
    pub fn word(self) -> i32 {
        match self {
            Self::Unused => -1,
            Self::RandomRange => -2,
            Self::Stat(stat) => i32::from(stat),
        }
    }
}

/// A drug (item subtype 2). Three slots share their selectors across the
/// immediate stage and two delayed stages; delayed amounts are deltas, so
/// a drug wears off only through opposite-signed delayed amounts
/// (SPEC-011 §3.3). Robotic and dead critters cannot take drugs (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrugSpec {
    pub stats: ItemStats,
    /// `Drug::stats`.
    pub slots: [DrugStat; 3],
    /// `Drug::immediate`.
    pub immediate: [i32; 3],
    /// `Drug::delay1`, game minutes after the dose, and `changes1`.
    pub first_delay: i32,
    pub first: [i32; 3],
    /// `Drug::delay2`, game minutes after the dose (not after stage 1),
    /// and `changes2`.
    pub second_delay: i32,
    pub second: [i32; 3],
    /// `Drug::addiction`: percent, rolled on every dose (SPEC-011 §3.4).
    pub addiction_chance: i32,
    /// `Drug::withdrawal`: a [`withdrawal_perk`] selector, or `None` (-1).
    pub withdrawal_perk: Option<i32>,
    /// `Drug::onset`: game minutes from addiction to withdrawal.
    pub withdrawal_onset: i32,
}

impl DrugSpec {
    /// A drug that changes one stat at once, with nothing delayed and no
    /// addiction.
    pub fn instant(stat: u8, amount: i32) -> Self {
        Self {
            stats: ItemStats::default(),
            slots: [DrugStat::Stat(stat), DrugStat::Unused, DrugStat::Unused],
            immediate: [amount, 0, 0],
            first_delay: 0,
            first: [0; 3],
            second_delay: 0,
            second: [0; 3],
            addiction_chance: 0,
            withdrawal_perk: None,
            withdrawal_onset: 0,
        }
    }
}

/// A drug (item subtype 2), extended flags 0 (drugs are used on others
/// by subtype).
pub fn drug(common: ProtoCommon, spec: &DrugSpec) -> Result<Prototype, EmitError> {
    for (slot, stat) in spec.slots.iter().enumerate() {
        match stat {
            DrugStat::RandomRange if slot != 0 => {
                return bad(format!(
                    "slot {slot}: only slot 0 can hold the random range"
                ));
            }
            DrugStat::Stat(stat) if *stat > drug_stat::RADIATION => {
                return bad(format!("slot {slot}: stat {stat} is outside 0 to 37"));
            }
            _ => {}
        }
    }
    if spec.first_delay < 0 || spec.second_delay < 0 || spec.withdrawal_onset < 0 {
        return bad("drug delays and the withdrawal onset must not be negative".into());
    }
    if !(0..=100).contains(&spec.addiction_chance) {
        return bad(format!(
            "addiction chance {} is outside 0 to 100",
            spec.addiction_chance
        ));
    }
    let data = ItemData::Drug(Drug {
        stats: spec.slots.map(DrugStat::word),
        immediate: spec.immediate,
        delay1: spec.first_delay,
        changes1: spec.first,
        delay2: spec.second_delay,
        changes2: spec.second,
        addiction: spec.addiction_chance,
        withdrawal: check_perk(spec.withdrawal_perk, "withdrawal")?,
        onset: spec.withdrawal_onset,
    });
    finish_item(&common, spec.stats, ExtendedFlags(0), data)
}

/// A key (item subtype 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub stats: ItemStats,
    /// `ItemData::Key::code`, copied to each placed key's own key word
    /// (SPEC-003 §2.11). The engine never compares it with a door's key
    /// code: unlocking is done by the door's script (SPEC-013 §3.8).
    pub code: i32,
}

/// A key (item subtype 6), extended flags 0.
pub fn key(common: ProtoCommon, spec: &KeySpec) -> Result<Prototype, EmitError> {
    finish_item(
        &common,
        spec.stats,
        ExtendedFlags(0),
        ItemData::Key { code: spec.code },
    )
}

/// A miscellaneous item's power source (`ItemData::Misc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSource {
    /// `power_pid`: the one ammunition PID the item accepts as charge
    /// (SPEC-011 §3.14).
    pub ammo: u32,
    /// `power_type`: the calibre of that ammunition (SPEC-024b names this
    /// word the calibre).
    pub caliber: i32,
}

/// A miscellaneous item (item subtype 5) with an optional power source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MiscSpec {
    pub stats: ItemStats,
    pub power: Option<PowerSource>,
    /// `charges`: the maximum charges; each placed item stores its
    /// current charges (SPEC-003 §2.11), which the interface bar's ammo
    /// bar shows (SPEC-014 §3.23). 0 or more.
    pub charges: i32,
    /// Extended flags to set, for example [`ExtendedFlags::USE`] for an
    /// item a script makes usable.
    pub extended: ExtendedFlags,
}

/// A miscellaneous item (item subtype 5) with power and charges:
/// [`crate::proto::misc_item`] is this with no power source, no charges
/// and no extended flags.
pub fn misc_item_with(common: ProtoCommon, spec: &MiscSpec) -> Result<Prototype, EmitError> {
    if spec.charges < 0 {
        return bad(format!("charges {} are negative", spec.charges));
    }
    let (power_pid, power_type) = match spec.power {
        None => (NO_PID, 0),
        Some(power) => (
            check_item_pid(Some(power.ammo), "power source")?,
            power.caliber,
        ),
    };
    let data = ItemData::Misc {
        power_pid,
        power_type,
        charges: spec.charges,
    };
    finish_item(&common, spec.stats, spec.extended, data)
}
