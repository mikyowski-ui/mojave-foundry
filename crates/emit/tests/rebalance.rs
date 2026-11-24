//! Rebalance tables: synthetic source records (a 10 mm pistol, a hunting
//! rifle, a minigun, a laser pistol, leather armour, a stimpak, .32
//! rounds, a raider) become specs that the constructors accept and that
//! round-trip through `vault-formats`. The numbers are invented for the
//! tests, not taken from any game.

use emit::animation::{AttackMode, WeaponCode};
use emit::fighter::fighter_prototype;
use emit::items::{DamageType, DrugStat, Skill, ammo, armor, armor_perk, drug, drug_stat, weapon};
use emit::proto::{self, Material, ObjectFlags, ProtoCommon, stat};
use emit::rebalance::{
    AidStat, Rebalance, SourceActor, SourceAid, SourceAmmo, SourceArmor, SourceDamage,
    SourceEffect, SourceGame, SourceSkill, SourceWeapon, WeaponClass, aid_spec, ammo_spec,
    armor_spec, fighter_spec, normalize_key, weapon_spec,
};
use vault_formats::pro::{Category, PrototypeData};
use vault_formats::{Fid, GameProfile, Prototype};

fn item(index: u32) -> ProtoCommon {
    ProtoCommon {
        pid: proto::pid(Category::Item, index).unwrap(),
        fid: Fid::new(0, index as u16, 0, 0, 0),
        message_base: 100 * index as i32,
        flags: ObjectFlags::DECAL,
        material: Material::Metal,
    }
}

fn round_trips(prototype: &Prototype) {
    let bytes = prototype.write();
    assert_eq!(
        &Prototype::parse_for(&bytes, GameProfile::Fallout2).unwrap(),
        prototype
    );
}

fn ten_mm() -> SourceWeapon {
    SourceWeapon {
        name: "Synthetic 10mm pistol".into(),
        class: Some(WeaponClass::Pistol),
        damage: 10.0,
        clip: 12,
        value: 200,
        weight: 3.0,
        ap: 18.0,
        range: 1200.0,
        caliber: Some("10mm".into()),
        ..SourceWeapon::default()
    }
}

fn hunting_rifle() -> SourceWeapon {
    SourceWeapon {
        name: "Synthetic hunting rifle".into(),
        class: Some(WeaponClass::Rifle),
        damage: 30.0,
        clip: 5,
        value: 300,
        weight: 9.0,
        ap: 24.0,
        range: 3000.0,
        min_spread: 0.2,
        caliber: Some(".32 caliber".into()),
        ..SourceWeapon::default()
    }
}

fn minigun() -> SourceWeapon {
    SourceWeapon {
        name: "Synthetic minigun".into(),
        class: Some(WeaponClass::Minigun),
        damage: 4.0,
        clip: 240,
        value: 1000,
        weight: 20.0,
        ap: 30.0,
        fire_rate: 25.0,
        automatic: true,
        caliber: Some("5mm".into()),
        strength: 8,
        ..SourceWeapon::default()
    }
}

fn laser_pistol() -> SourceWeapon {
    SourceWeapon {
        name: "Synthetic laser pistol".into(),
        class: Some(WeaponClass::EnergyPistol),
        damage: 14.0,
        damage_kind: SourceDamage::Laser,
        clip: 30,
        value: 250,
        weight: 2.0,
        ap: 16.0,
        caliber: Some("Energy Cell".into()),
        ..SourceWeapon::default()
    }
}

#[test]
fn weapons_become_valid_fallout_2_weapons() {
    for game in [SourceGame::Fallout3, SourceGame::NewVegas] {
        let rebalance = Rebalance::for_game(game);
        for (index, source) in [ten_mm(), hunting_rifle(), minigun(), laser_pistol()]
            .iter()
            .enumerate()
        {
            let spec = weapon_spec(source, &rebalance);
            let prototype = weapon(item(index as u32 + 1), &spec)
                .unwrap_or_else(|error| panic!("{}: {error}", source.name));
            round_trips(&prototype);
            assert!(spec.damage[0] >= 1 && spec.damage[0] <= spec.damage[1]);
            assert!(spec.ap_cost[0] >= 1 && spec.ap_cost[0] <= 9);
        }
    }
    let fo3 = Rebalance::fallout3_default();
    let pistol = weapon_spec(&ten_mm(), &fo3);
    assert_eq!(pistol.code, WeaponCode::Pistol);
    assert_eq!(pistol.modes, [AttackMode::Single, AttackMode::None]);
    assert_eq!(pistol.damage, [6, 14]);
    assert_eq!(pistol.ap_cost[0], 5); // 18 / 4, rounded
    assert_eq!(pistol.ranges[0], 19); // 1200 / 64, rounded
    assert_eq!((pistol.capacity, pistol.caliber), (12, 101));
    assert_eq!(pistol.skill(0), Some(Skill::SmallGuns));
    assert_eq!((pistol.stats.cost, pistol.stats.weight), (200, 3));

    let rifle = weapon_spec(&hunting_rifle(), &fo3);
    assert_eq!(rifle.code, WeaponCode::Shotgun);
    assert!(rifle.two_handed);
    assert_eq!(rifle.caliber, 103);
    assert_eq!(rifle.perk, None);
    let mut accurate = fo3.clone();
    accurate.accurate_spread_below = Some(0.5);
    assert_eq!(
        weapon_spec(&hunting_rifle(), &accurate).perk,
        Some(emit::items::weapon_perk::ACCURATE)
    );

    let mini = weapon_spec(&minigun(), &fo3);
    assert_eq!(mini.modes[0], AttackMode::Burst);
    assert_eq!(mini.burst, 25);
    assert!(mini.big_gun);
    assert_eq!(mini.skill(0), Some(Skill::BigGuns));
    assert_eq!(mini.min_strength, 8);
    assert_eq!(mini.ap_cost[0], 9); // 30 / 4 + 1, rounded

    let laser = weapon_spec(&laser_pistol(), &fo3);
    assert_eq!(laser.damage_type, DamageType::Laser);
    assert_eq!(laser.skill(0), Some(Skill::EnergyWeapons));
    assert_eq!(laser.caliber, 108);
    // An energy class with untyped damage still uses Energy Weapons.
    let untyped = SourceWeapon {
        damage_kind: SourceDamage::Ballistic,
        ..laser_pistol()
    };
    assert_eq!(
        weapon_spec(&untyped, &fo3).skill(0),
        Some(Skill::EnergyWeapons)
    );
    // New Vegas scales damage down; an automatic rifle gains burst.
    let nv = Rebalance::new_vegas_default();
    assert!(weapon_spec(&ten_mm(), &nv).damage[1] < pistol.damage[1]);
    let automatic = SourceWeapon {
        automatic: true,
        ..ten_mm()
    };
    let spec = weapon_spec(&automatic, &fo3);
    assert_eq!(spec.modes, [AttackMode::Single, AttackMode::Burst]);
    assert!(spec.burst >= 2 && spec.burst <= spec.capacity);
    // Unknown calibre and class fall back to the documented defaults.
    let unknown = SourceWeapon {
        class: None,
        caliber: Some("nothing known".into()),
        ..ten_mm()
    };
    let spec = weapon_spec(&unknown, &fo3);
    assert_eq!((spec.code, spec.caliber), (WeaponCode::Pistol, 199));
}

#[test]
fn every_class_yields_a_weapon_the_constructor_accepts() {
    for game in [SourceGame::Fallout3, SourceGame::NewVegas] {
        let rebalance = Rebalance::for_game(game);
        for class in WeaponClass::ALL {
            for source in [
                SourceWeapon {
                    class: Some(class),
                    ..SourceWeapon::default()
                },
                SourceWeapon {
                    class: Some(class),
                    damage: 500.0,
                    clip: 1,
                    ap: 400.0,
                    fire_rate: 100.0,
                    automatic: true,
                    range: 1.0e9,
                    weight: -3.0,
                    value: -5,
                    strength: 40,
                    ..SourceWeapon::default()
                },
            ] {
                let spec = weapon_spec(&source, &rebalance);
                weapon(item(1), &spec).unwrap_or_else(|error| panic!("{class:?}: {error}"));
            }
        }
    }
}

#[test]
fn armour_follows_resistance_or_threshold() {
    let leather = SourceArmor {
        name: "Synthetic leather armour".into(),
        dr: 12.0,
        dt: 10.0,
        value: 160,
        weight: 15.0,
        power_armor: false,
    };
    let fo3 = armor_spec(
        &SourceArmor {
            dt: 0.0,
            ..leather.clone()
        },
        &Rebalance::fallout3_default(),
    );
    assert_eq!(fo3.armor_class, 12);
    assert_eq!(fo3.resistances[0], 24);
    assert_eq!(fo3.thresholds[0], 2); // 0.15 x 12
    assert_eq!(fo3.resistances[DamageType::Emp.index()], 100);
    assert_eq!(fo3.thresholds[DamageType::Emp.index()], 0);
    let nv = armor_spec(
        &SourceArmor {
            dr: 0.0,
            ..leather.clone()
        },
        &Rebalance::new_vegas_default(),
    );
    assert_eq!(nv.thresholds[0], 3); // 0.3 x 10
    assert_eq!(nv.resistances[0], 25); // 2.5 x 10
    assert_eq!(nv.armor_class, 12);
    for spec in [&fo3, &nv] {
        round_trips(&armor(item(1), spec).unwrap());
        assert!(spec.resistances.iter().all(|r| (0..=100).contains(r)));
    }
    let power = armor_spec(
        &SourceArmor {
            power_armor: true,
            dr: 300.0,
            ..leather
        },
        &Rebalance::fallout3_default(),
    );
    assert_eq!(power.perk, Some(armor_perk::POWERED_ARMOR));
    assert_eq!(power.resistances[0], 90);
}

#[test]
fn ammunition_and_aid() {
    let rounds = SourceAmmo {
        name: "Synthetic .32 rounds".into(),
        caliber: ".32".into(),
        value: 1.5,
        weight: 0.0,
        damage_mult: 1.5,
        ..SourceAmmo::default()
    };
    let spec = ammo_spec(&rounds, &Rebalance::fallout3_default());
    assert_eq!((spec.caliber, spec.quantity), (103, 20));
    assert_eq!((spec.multiplier, spec.divisor), (3, 2));
    assert_eq!(spec.stats.cost, 30);
    round_trips(&ammo(item(1), &spec).unwrap());
    // The rifle and the rounds share a calibre number, so reload accepts it.
    assert_eq!(
        weapon_spec(&hunting_rifle(), &Rebalance::fallout3_default()).caliber,
        spec.caliber
    );
    assert_eq!(normalize_key(" .32 Caliber "), "32caliber");

    let stimpak = SourceAid {
        name: "Synthetic stimpak".into(),
        value: 50,
        weight: 0.0,
        health: 40,
        ..SourceAid::default()
    };
    let spec = aid_spec(&stimpak, &Rebalance::fallout3_default());
    assert_eq!(spec.slots[0], DrugStat::Stat(drug_stat::CURRENT_HP));
    assert_eq!(spec.immediate[0], 20);
    assert_eq!((spec.first_delay, spec.addiction_chance), (0, 0));
    round_trips(&drug(item(1), &spec).unwrap());

    let chem = SourceAid {
        name: "Synthetic strength chem".into(),
        radiation: 10,
        effects: vec![
            SourceEffect {
                stat: AidStat::Special(0),
                amount: 2,
                seconds: 240.0,
            },
            SourceEffect {
                stat: AidStat::DamageResistance,
                amount: 25,
                seconds: 60.0,
            },
            SourceEffect {
                stat: AidStat::ActionPoints,
                amount: 1,
                seconds: 0.0,
            },
        ],
        addiction_chance: 0.25,
        ..SourceAid::default()
    };
    let spec = aid_spec(&chem, &Rebalance::fallout3_default());
    assert_eq!(
        spec.slots,
        [
            DrugStat::Stat(drug_stat::RADIATION),
            DrugStat::Stat(0),
            DrugStat::Stat(24)
        ]
    );
    assert_eq!(spec.immediate, [-5, 2, 25]);
    assert_eq!((spec.first_delay, spec.first), (120, [0, -2, -25]));
    assert_eq!(spec.addiction_chance, 25);
    assert_eq!(spec.withdrawal_perk, Some(53));
    round_trips(&drug(item(1), &spec).unwrap());
}

#[test]
fn a_raider_becomes_a_fighter_with_its_armour_in_its_prototype() {
    let raider = SourceActor {
        name: "Synthetic raider".into(),
        level: 4,
        health: 60,
        special: [6, 5, 5, 3, 4, 6, 4],
        skills: vec![
            (SourceSkill::SmallGuns, 45),
            (SourceSkill::MeleeWeapons, 50),
            (SourceSkill::Guns, 80),
        ],
        action_points: 70,
        female: false,
    };
    let leather = armor_spec(
        &SourceArmor {
            dr: 12.0,
            ..SourceArmor::default()
        },
        &Rebalance::fallout3_default(),
    );
    for game in [SourceGame::Fallout3, SourceGame::NewVegas] {
        let rebalance = Rebalance::for_game(game);
        let spec = fighter_spec(&raider, Some(&leather), &rebalance, 3, 7);
        let common = ProtoCommon {
            pid: 0x0100_0001,
            fid: Fid::new(1, 1, 0, 0, 0),
            message_base: 100,
            flags: ObjectFlags::SOLID,
            material: Material::Stone,
        };
        let prototype = fighter_prototype(common, &spec).unwrap();
        round_trips(&prototype);
        let PrototypeData::Critter(critter) = &prototype.data else {
            panic!("a critter")
        };
        let effective = |index: usize| critter.base_stats[index] + critter.bonus_stats[index];
        assert_eq!(effective(stat::MAX_HIT_POINTS), spec.critter.hit_points);
        assert_eq!(effective(stat::MAX_ACTION_POINTS), 7);
        assert_eq!(effective(stat::ARMOR_CLASS), 6 + leather.armor_class);
        assert_eq!(critter.base_stats[24], leather.resistances[0]);
        assert_eq!((critter.ai, critter.team), (3, 7));
        assert_eq!(critter.experience, 25 + 4 * 10);
        // The base stats are what the engine's recomputation gives.
        assert_eq!(critter.base_stats[stat::MAX_HIT_POINTS], 15 + 6 + 2 * 5);
        match game {
            SourceGame::Fallout3 => {
                assert_eq!(spec.critter.hit_points, 34); // 60 x 0.5 + 4
                assert_eq!(spec.effective_skill(Skill::SmallGuns), 45);
                assert_eq!(spec.skills[Skill::BigGuns.index()], None);
            }
            SourceGame::NewVegas => {
                assert_eq!(spec.critter.hit_points, 28); // 60 x 0.4 + 4
                // Guns 80 sets both gun skills; the higher value wins.
                assert_eq!(spec.effective_skill(Skill::SmallGuns), 80);
                assert_eq!(spec.effective_skill(Skill::BigGuns), 80);
            }
        }
        // Melee 50 is below the untrained base (55 + 6): no points.
        assert_eq!(critter.skills[Skill::MeleeWeapons.index()], 0);
    }
}
