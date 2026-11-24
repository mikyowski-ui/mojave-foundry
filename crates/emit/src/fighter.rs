//! Critter prototypes for critters that fight: target hit points, AP, AC,
//! skills and protection, stored so that they hold whether or not the
//! engine recomputes derived stats.
//!
//! The engine recomputes a critter's derived **base** stats from its
//! SPECIAL whenever a primary stat changes (SPEC-006 §3.1): maximum HP
//! `15 + ST + 2 EN`, maximum AP `5 + AG / 2`, AC `AG`, melee damage
//! `max(ST - 5, 1)`, carry weight, sequence, healing rate, critical chance,
//! radiation and poison resistance. Thresholds, resistances, unarmed
//! damage, age and gender keep their stored values. So
//! [`fighter_prototype`] stores those formulas as the base stats and puts
//! every difference from a target in the **bonus** array, which
//! recomputation never touches: a target survives a drug, a perk or a
//! level change.
//!
//! A critter's skill words are invested points added to the untrained
//! base (SPEC-006 §3.3); a negative word is returned as the skill itself,
//! so points are never below 0. The AI uses a weapon only when the skill
//! its mode implies is at least its packet's `min_to_hit` (SPEC-009 §3.9).
//!
//! A non-player critter's protection is its prototype's alone: worn
//! armour adds nothing (SPEC-011 §3.10). Put the armour's DR and DT in
//! [`FighterSpec::resistances`] and [`FighterSpec::thresholds`] and its AC
//! in the target.

use crate::EmitError;
use crate::items::{DamageType, Skill};
use crate::proto::{
    ART_CRITTERS, BODY_BIPED, BODY_ROBOTIC, CritterSpec, KILL_CATEGORY_MAX, NO_HEAD, ProtoCommon,
    check_common, finish, stat,
};
use vault_formats::Prototype;
use vault_formats::pro::{Category, Critter, CritterFlags, ExtendedFlags, PrototypeData};

/// A fighting critter's figures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FighterSpec {
    /// AI packet, team, SPECIAL, sex, age, experience award, body type
    /// and kill category, as [`CritterSpec`]; its `hit_points` is the
    /// target maximum HP.
    pub critter: CritterSpec,
    /// Target maximum AP; `None` keeps `5 + AG / 2`.
    pub action_points: Option<i32>,
    /// Target armour class before the unspent-AP bonus; `None` keeps `AG`.
    pub armor_class: Option<i32>,
    /// Target melee damage; `None` keeps `max(ST - 5, 1)`.
    pub melee_damage: Option<i32>,
    /// Target effective skill percentages (0 to 200); `None` keeps the
    /// untrained base.
    pub skills: [Option<i32>; 18],
    /// DT by damage type (stats 17 to 23).
    pub thresholds: [i32; 7],
    /// DR percent by damage type (stats 24 to 30).
    pub resistances: [i32; 7],
    /// The unarmed damage type, the Fallout 2 record's last word
    /// (SPEC-016 §3.1).
    pub unarmed_damage: DamageType,
    /// Critter behaviour flags (SPEC-003 §2.6), such as
    /// `CritterFlags::NO_STEAL`.
    pub flags: CritterFlags,
}

impl FighterSpec {
    /// `critter` with no targets beyond its hit points, no protection,
    /// normal unarmed damage, no flags.
    pub fn new(critter: CritterSpec) -> Self {
        Self {
            critter,
            action_points: None,
            armor_class: None,
            melee_damage: None,
            skills: [None; 18],
            thresholds: [0; 7],
            resistances: [0; 7],
            unarmed_damage: DamageType::Normal,
            flags: CritterFlags(0),
        }
    }

    /// The 35 base stats the engine's recomputation gives for this SPECIAL
    /// (SPEC-006 §3.1), with the stored thresholds, resistances, age and
    /// gender.
    pub fn base_stats(&self) -> [i32; stat::COUNT] {
        let c = &self.critter;
        let [st, pe, en, _, _, ag, lk] = c.special;
        let mut stats = [0; stat::COUNT];
        stats[..7].copy_from_slice(&c.special);
        stats[stat::MAX_HIT_POINTS] = 15 + st + 2 * en;
        stats[stat::MAX_ACTION_POINTS] = 5 + ag / 2;
        stats[stat::ARMOR_CLASS] = ag;
        stats[stat::MELEE_DAMAGE] = (st - 5).max(1);
        stats[stat::CARRY_WEIGHT] = 25 + 25 * st;
        stats[stat::SEQUENCE] = 2 * pe;
        stats[stat::HEALING_RATE] = (en / 3).max(1);
        stats[stat::CRITICAL_CHANCE] = lk;
        stats[17..24].copy_from_slice(&self.thresholds);
        stats[24..31].copy_from_slice(&self.resistances);
        stats[stat::RADIATION_RESISTANCE] = 2 * en;
        stats[stat::POISON_RESISTANCE] = 5 * en;
        stats[stat::AGE] = c.age;
        stats[stat::GENDER] = i32::from(c.female);
        stats
    }

    /// The bonus stats that bring [`FighterSpec::base_stats`] to the
    /// targets.
    pub fn bonus_stats(&self) -> [i32; stat::COUNT] {
        let base = self.base_stats();
        let mut bonus = [0; stat::COUNT];
        bonus[stat::MAX_HIT_POINTS] = self.critter.hit_points - base[stat::MAX_HIT_POINTS];
        let mut target = |index: usize, value: Option<i32>| {
            if let Some(value) = value {
                bonus[index] = value - base[index];
            }
        };
        target(stat::MAX_ACTION_POINTS, self.action_points);
        target(stat::ARMOR_CLASS, self.armor_class);
        target(stat::MELEE_DAMAGE, self.melee_damage);
        bonus
    }

    /// The invested skill points: target less untrained base, at least 0.
    pub fn skill_points(&self) -> [i32; 18] {
        let mut points = [0; 18];
        for skill in Skill::ALL {
            if let Some(target) = self.skills[skill.index()] {
                points[skill.index()] = (target - skill.base(self.critter.special)).max(0);
            }
        }
        points
    }

    /// The effective skill percentage the engine computes for a
    /// non-player critter with these points (no tags, traits or perks).
    pub fn effective_skill(&self, skill: Skill) -> i32 {
        (skill.base(self.critter.special) + self.skill_points()[skill.index()]).min(200)
    }

    fn check(&self) -> Result<(), EmitError> {
        self.critter.check()?;
        let bad = |message: String| Err(EmitError::Critter(message));
        if self.action_points.is_some_and(|ap| !(1..=99).contains(&ap)) {
            return bad(format!(
                "action points {:?} are outside 1 to 99",
                self.action_points
            ));
        }
        if self.armor_class.is_some_and(|ac| ac < 0) || self.melee_damage.is_some_and(|m| m < 0) {
            return bad("armour class and melee damage must not be negative".into());
        }
        if let Some(value) = self
            .skills
            .iter()
            .flatten()
            .find(|v| !(0..=200).contains(*v))
        {
            return bad(format!("skill {value} is outside 0 to 200"));
        }
        if self
            .thresholds
            .iter()
            .chain(&self.resistances)
            .any(|v| *v < 0)
        {
            return bad("thresholds and resistances must not be negative".into());
        }
        if !(BODY_BIPED..=BODY_ROBOTIC).contains(&self.critter.body)
            || !(0..=KILL_CATEGORY_MAX).contains(&self.critter.kill_category())
        {
            return bad("body type or kill category out of range".into());
        }
        Ok(())
    }
}

/// A Fallout 2 critter prototype (416 bytes) for `spec`: base stats from
/// [`FighterSpec::base_stats`], bonus stats from
/// [`FighterSpec::bonus_stats`], skill points from
/// [`FighterSpec::skill_points`], no talking head, no script, extended
/// flags 0. Otherwise as [`crate::proto::critter_prototype`].
pub fn fighter_prototype(common: ProtoCommon, spec: &FighterSpec) -> Result<Prototype, EmitError> {
    check_common(&common, Category::Critter, ART_CRITTERS)?;
    spec.check()?;
    let c = &spec.critter;
    Ok(finish(
        &common,
        Category::Critter,
        ExtendedFlags(0),
        PrototypeData::Critter(Box::new(Critter {
            head_art: NO_HEAD,
            ai: c.ai_packet,
            team: c.team,
            flags: spec.flags,
            base_stats: spec.base_stats(),
            bonus_stats: spec.bonus_stats(),
            skills: spec.skill_points(),
            body: c.body,
            experience: c.experience,
            kill_category: c.kill_category(),
            damage_type: Some(spec.unarmed_damage as u32),
        })),
    ))
}
