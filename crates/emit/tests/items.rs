//! Item and transit prototypes: every constructor's output parses back
//! equal through `Prototype::parse_for` under both games, each spec field
//! lands in the word the format keeps it in, and what the format or the
//! engine cannot use is refused. All values are synthetic.

use emit::animation::{AttackMode, WeaponCode};
use emit::items::{
    AmmoSpec, ArmorSpec, DamageType, DrugSpec, DrugStat, KeySpec, MiscSpec, PowerSource, Skill,
    WeaponSpec, ammo, armor, armor_perk, drug, drug_stat, implied_skill, key, misc_item_with,
    weapon, weapon_perk, withdrawal_perk,
};
use emit::msg::MSG_FIELD_BYTES;
use emit::proto::{
    self, DoorSpec, ItemStats, Material, ObjectFlags, ProtoCommon, check_prototype, door, misc_item,
};
use emit::text::{ProtoText, describe, fold_text};
use emit::transit::{Destination, GROUND_REACH, LadderKind, elevator, ladder, stairs};
use emit::{FormatWriters, LocalWriters, MsgFile};
use vault_formats::pro::{
    Category, ExtendedFlags, ItemData, LockFlags, PrototypeData, SceneryData,
};
use vault_formats::{Fid, GameProfile, MsgCatalog, Prototype};

fn common(category: Category, index: u32) -> ProtoCommon {
    let art = match category {
        Category::Scenery => 2,
        _ => 0,
    };
    ProtoCommon {
        pid: proto::pid(category, index).unwrap(),
        fid: Fid::new(art, index as u16, 0, 0, 0),
        message_base: proto::default_message_base(index).unwrap(),
        flags: ObjectFlags::DECAL,
        material: Material::Metal,
    }
}

fn stats(size: i32, weight: i32, cost: i32) -> ItemStats {
    ItemStats {
        size,
        weight,
        cost,
        inventory_art: Some(Fid::new(7, 40, 0, 0, 0)),
        sound: b'A',
    }
}

fn pistol() -> WeaponSpec {
    WeaponSpec {
        stats: stats(2, 3, 250),
        caliber: 101,
        ammo: Some(proto::pid(Category::Item, 9).unwrap()),
        capacity: 12,
        min_strength: 3,
        sound: b'P',
        ..WeaponSpec::new(
            WeaponCode::Pistol,
            [5, 12],
            DamageType::Normal,
            AttackMode::Single,
            5,
            20,
        )
    }
}

fn smg() -> WeaponSpec {
    WeaponSpec {
        modes: [AttackMode::Single, AttackMode::Burst],
        ap_cost: [5, 6],
        ranges: [25, 18],
        burst: 10,
        capacity: 30,
        caliber: 101,
        two_handed: true,
        perk: Some(weapon_perk::ACCURATE),
        critical_failure: 3,
        projectile: Some(proto::pid(Category::Misc, 30).unwrap()),
        ..WeaponSpec::new(
            WeaponCode::Smg,
            [6, 14],
            DamageType::Normal,
            AttackMode::Single,
            5,
            25,
        )
    }
}

fn knife() -> WeaponSpec {
    WeaponSpec {
        modes: [AttackMode::Swing, AttackMode::Thrust],
        ap_cost: [3, 4],
        ranges: [1, 1],
        ..WeaponSpec::new(
            WeaponCode::Knife,
            [1, 6],
            DamageType::Normal,
            AttackMode::Swing,
            3,
            1,
        )
    }
}

fn leather() -> ArmorSpec {
    ArmorSpec {
        stats: stats(10, 15, 700),
        armor_class: 15,
        resistances: [25, 20, 20, 10, 30, 100, 20],
        thresholds: [2, 0, 0, 0, 0, 0, 0],
        perk: None,
        male_art: Some(Fid::new(1, 12, 0, 0, 0)),
        female_art: Some(Fid::new(1, 13, 0, 0, 0)),
    }
}

fn rounds() -> AmmoSpec {
    AmmoSpec {
        stats: stats(1, 1, 50),
        caliber: 101,
        quantity: 24,
        ac_modifier: 0,
        dr_modifier: -10,
        multiplier: 3,
        divisor: 2,
    }
}

fn healer() -> DrugSpec {
    DrugSpec {
        stats: stats(1, 0, 175),
        slots: [
            DrugStat::Stat(drug_stat::CURRENT_HP),
            DrugStat::Stat(0),
            DrugStat::Unused,
        ],
        immediate: [12, 2, 0],
        first_delay: 120,
        first: [0, -2, 0],
        second_delay: 0,
        second: [0; 3],
        addiction_chance: 10,
        withdrawal_perk: Some(withdrawal_perk::STRENGTH_ENDURANCE_AGILITY),
        withdrawal_onset: 2880,
    }
}

fn all_items() -> Vec<Prototype> {
    vec![
        weapon(common(Category::Item, 1), &pistol()).unwrap(),
        weapon(common(Category::Item, 2), &smg()).unwrap(),
        weapon(common(Category::Item, 3), &knife()).unwrap(),
        armor(common(Category::Item, 4), &leather()).unwrap(),
        ammo(common(Category::Item, 5), &rounds()).unwrap(),
        drug(common(Category::Item, 6), &healer()).unwrap(),
        key(
            common(Category::Item, 7),
            &KeySpec {
                stats: stats(1, 0, 0),
                code: 4242,
            },
        )
        .unwrap(),
        misc_item_with(
            common(Category::Item, 8),
            &MiscSpec {
                stats: stats(1, 2, 90),
                power: Some(PowerSource {
                    ammo: proto::pid(Category::Item, 5).unwrap(),
                    caliber: 101,
                }),
                charges: 40,
                extended: ExtendedFlags::USE,
            },
        )
        .unwrap(),
    ]
}

fn all_scenery() -> Vec<Prototype> {
    let destination = Destination {
        map: 7,
        hex: 12345,
        elevation: 1,
        orientation: 2,
    };
    vec![
        stairs(common(Category::Scenery, 1), destination, true, b'S').unwrap(),
        ladder(
            common(Category::Scenery, 2),
            LadderKind::Up,
            destination,
            false,
            0,
        )
        .unwrap(),
        ladder(
            common(Category::Scenery, 3),
            LadderKind::Down,
            destination,
            true,
            0,
        )
        .unwrap(),
        elevator(common(Category::Scenery, 4), 3, 1, 0).unwrap(),
        door(
            common(Category::Scenery, 5),
            DoorSpec {
                locked: true,
                jammed: true,
                key: 4242,
                sound: b'B',
                ..DoorSpec::default()
            },
        )
        .unwrap(),
    ]
}

#[test]
fn every_new_prototype_round_trips_under_both_games() {
    for prototype in all_items().into_iter().chain(all_scenery()) {
        check_prototype(&prototype).unwrap();
        let bytes = LocalWriters.pro(&prototype).unwrap();
        assert_eq!(bytes.len(), prototype.consumed_len);
        for game in [GameProfile::Fallout1, GameProfile::Fallout2] {
            assert_eq!(Prototype::parse_for(&bytes, game).unwrap(), prototype);
        }
    }
}

#[test]
fn item_payloads_have_the_spec_lengths() {
    // SPEC-003 §2.3: reader-consumed totals by subtype.
    let lengths: Vec<usize> = all_items().iter().map(|p| p.write().len()).collect();
    assert_eq!(lengths, [122, 122, 122, 129, 81, 125, 61, 69]);
    let lengths: Vec<usize> = all_scenery().iter().map(|p| p.write().len()).collect();
    // Stairs, elevator and door 49; ladders 45 (SPEC-003 §2.5).
    assert_eq!(lengths, [49, 45, 45, 49, 49]);
}

#[test]
fn weapon_fields_land_where_the_format_keeps_them() {
    let items = all_items();
    let PrototypeData::Item(item) = &items[1].data else {
        panic!("an item")
    };
    let ItemData::Weapon(w) = &item.data else {
        panic!("a weapon")
    };
    assert_eq!(w.animation, 6);
    assert_eq!(w.damage, [6, 14]);
    assert_eq!(w.ap_cost, [5, 6]);
    assert_eq!(w.ranges, [25, 18]);
    assert_eq!((w.burst, w.capacity, w.caliber), (10, 30, 101));
    assert_eq!(w.perk, 59);
    assert_eq!(w.critical_failure, 3);
    assert_eq!(w.projectile_pid, 0x0500_001e);
    assert_eq!(w.ammo_pid, -1);
    let extended = items[1].extended;
    assert_eq!(extended.attack_modes(), [6, 7]);
    assert!(extended.contains(ExtendedFlags::TWO_HANDED));
    assert!(!extended.contains(ExtendedFlags::BIG_GUN));
    assert!(items[1].permits_pickup());
    // Byte 0x38 is the item sound letter; the weapon sound is the last byte.
    let bytes = items[0].write();
    assert_eq!(bytes[0x38], b'A');
    assert_eq!(*bytes.last().unwrap(), b'P');
    // Minimum then maximum damage, in reader order.
    assert_eq!(&bytes[0x39 + 4..0x39 + 12], &[0, 0, 0, 5, 0, 0, 0, 12]);
    assert_eq!(item.inventory_art, u32::MAX);
    let PrototypeData::Item(first) = &items[0].data else {
        panic!("an item")
    };
    assert_eq!(first.inventory_art, 0x0700_0028);
}

#[test]
fn the_skill_is_implied_by_mode_damage_and_the_big_gun_flag() {
    assert_eq!(pistol().skill(0), Some(Skill::SmallGuns));
    assert_eq!(pistol().skill(1), None);
    assert_eq!(knife().skill(0), Some(Skill::MeleeWeapons));
    let mut laser = pistol();
    laser.damage_type = DamageType::Laser;
    assert_eq!(laser.skill(0), Some(Skill::EnergyWeapons));
    let mut big = smg();
    big.big_gun = true;
    assert_eq!(big.skill(1), Some(Skill::BigGuns));
    big.damage_type = DamageType::Plasma;
    assert_eq!(big.skill(1), Some(Skill::EnergyWeapons));
    // Fire is not energy: a flamer with the big-gun flag is Big Guns.
    assert_eq!(
        implied_skill(AttackMode::Burst, DamageType::Fire, true),
        Some(Skill::BigGuns)
    );
    assert_eq!(
        implied_skill(AttackMode::Kick, DamageType::Laser, true),
        Some(Skill::Unarmed)
    );
    assert_eq!(
        implied_skill(AttackMode::Throw, DamageType::Explosion, false),
        Some(Skill::Throwing)
    );
    assert_eq!(
        implied_skill(AttackMode::None, DamageType::Normal, false),
        None
    );
}

#[test]
fn armour_ammo_drug_key_and_misc_fields() {
    let items = all_items();
    let data = |index: usize| match &items[index].data {
        PrototypeData::Item(item) => item.data.clone(),
        _ => panic!("an item"),
    };
    let ItemData::Armor(a) = data(3) else {
        panic!()
    };
    assert_eq!(a.armor_class, 15);
    assert_eq!(a.resistances[5], 100);
    assert_eq!(
        (a.male_art, a.female_art, a.perk),
        (0x0100_000c, 0x0100_000d, -1)
    );
    assert_eq!(
        data(4),
        ItemData::Ammunition {
            caliber: 101,
            quantity: 24,
            armor_modifier: 0,
            resistance_modifier: -10,
            multiplier: 3,
            divisor: 2,
        }
    );
    let ItemData::Drug(d) = data(5) else { panic!() };
    assert_eq!(d.stats, [35, 0, -1]);
    assert_eq!((d.delay1, d.changes1), (120, [0, -2, 0]));
    assert_eq!((d.addiction, d.withdrawal, d.onset), (10, 54, 2880));
    assert!(items[5].permits_use_on());
    assert_eq!(data(6), ItemData::Key { code: 4242 });
    assert_eq!(
        data(7),
        ItemData::Misc {
            power_pid: 0x0000_0005,
            power_type: 101,
            charges: 40
        }
    );
    assert!(items[7].permits_use());
    // The existing misc constructor is the no-power case.
    let plain = misc_item(common(Category::Item, 8), stats(1, 2, 90)).unwrap();
    let mut spec = MiscSpec {
        stats: stats(1, 2, 90),
        ..MiscSpec::default()
    };
    assert_eq!(
        misc_item_with(common(Category::Item, 8), &spec).unwrap(),
        plain
    );
    spec.charges = -1;
    assert!(misc_item_with(common(Category::Item, 8), &spec).is_err());
}

#[test]
fn transit_and_door_fields() {
    let scenery = all_scenery();
    let data = |index: usize| match &scenery[index].data {
        PrototypeData::Scenery(s) => s.data.clone(),
        _ => panic!("scenery"),
    };
    let packed = 12345 | (2 << 26) | (1 << 29);
    assert_eq!(
        data(0),
        SceneryData::Stairs {
            map: 7,
            destination: packed
        }
    );
    assert_eq!(
        data(1),
        SceneryData::LadderUp {
            destination: packed
        }
    );
    assert_eq!(
        data(2),
        SceneryData::LadderDown {
            destination: packed
        }
    );
    assert_eq!(data(3), SceneryData::Elevator { kind: 3, level: 1 });
    assert_eq!(
        data(4),
        SceneryData::Door {
            state: LockFlags(LockFlags::LOCKED.0 | LockFlags::JAMMED.0),
            key: 4242
        }
    );
    for prototype in &scenery {
        assert!(prototype.permits_use());
    }
    assert_eq!(scenery[0].extended.0 & GROUND_REACH, GROUND_REACH);
    assert_eq!(scenery[1].extended.0 & GROUND_REACH, 0);
    let far = Destination {
        map: 0,
        hex: 40_000,
        elevation: 0,
        orientation: 0,
    };
    assert!(stairs(common(Category::Scenery, 1), far, false, 0).is_err());
    assert!(elevator(common(Category::Scenery, 4), 12, 0, 0).is_err());
}

#[test]
fn constructors_refuse_what_the_engine_cannot_use() {
    let at = || common(Category::Item, 1);
    let refuse = |change: &dyn Fn(&mut WeaponSpec)| {
        let mut spec = pistol();
        change(&mut spec);
        assert!(weapon(at(), &spec).is_err(), "{spec:?}");
    };
    refuse(&|s| s.damage = [9, 3]);
    refuse(&|s| s.modes = [AttackMode::None, AttackMode::Single]);
    refuse(&|s| s.ap_cost = [10, 0]);
    refuse(&|s| s.ap_cost = [0, 0]);
    refuse(&|s| s.ranges = [0, 0]);
    refuse(&|s| s.capacity = 0);
    refuse(&|s| s.caliber = 0);
    refuse(&|s| s.min_strength = 11);
    refuse(&|s| s.critical_failure = 7);
    refuse(&|s| s.perk = Some(64));
    refuse(&|s| s.sound = b'}');
    refuse(&|s| s.ammo = Some(0x0500_0001));
    refuse(&|s| s.projectile = Some(0x0000_0001));
    refuse(&|s| s.code = WeaponCode::Unarmed);
    refuse(&|s| {
        s.modes[1] = AttackMode::Burst;
        s.ap_cost[1] = 6;
        s.ranges[1] = 10;
        s.burst = 1;
    });
    // A weapon without a magazine takes no ammunition.
    let mut spec = knife();
    spec.ammo = Some(0x0000_0001);
    assert!(weapon(at(), &spec).is_err());
    // Scenery art for an item, an item PID with scenery art.
    let mut wrong = at();
    wrong.fid = Fid::new(2, 1, 0, 0, 0);
    assert!(weapon(wrong, &pistol()).is_err());
    let mut negative = leather();
    negative.resistances[0] = -1;
    assert!(armor(at(), &negative).is_err());
    let mut figure = leather();
    figure.male_art = Some(Fid::new(1, 12, 1, 0, 0));
    assert!(armor(at(), &figure).is_err());
    for bad in [
        AmmoSpec {
            quantity: 0,
            ..rounds()
        },
        AmmoSpec {
            divisor: 0,
            ..rounds()
        },
        AmmoSpec {
            caliber: 0,
            ..rounds()
        },
    ] {
        assert!(ammo(at(), &bad).is_err());
    }
    let mut random = healer();
    random.slots[1] = DrugStat::RandomRange;
    assert!(drug(at(), &random).is_err());
    let mut stat = healer();
    stat.slots[2] = DrugStat::Stat(38);
    assert!(drug(at(), &stat).is_err());
    let mut chance = healer();
    chance.addiction_chance = 101;
    assert!(drug(at(), &chance).is_err());
    let mut first = healer();
    first.slots[0] = DrugStat::RandomRange;
    assert!(drug(at(), &first).is_ok());
    let mut stats_bad = pistol();
    stats_bad.stats.weight = -1;
    assert!(weapon(at(), &stats_bad).is_err());
    let powered = ArmorSpec {
        perk: Some(armor_perk::POWERED_ARMOR),
        ..leather()
    };
    assert!(armor(at(), &powered).is_ok());
}

#[test]
fn names_fold_to_what_the_message_format_carries() {
    assert_eq!(
        fold_text("Café “Nuka” — 10 mm", 100),
        b"Cafe \"Nuka\" - 10 mm"
    );
    assert_eq!(fold_text("  a {b}\nc\td  ", 100), b"a (b) c d");
    assert_eq!(fold_text("Œuvre straße ½", 100), b"OEuvre strasse 1/2");
    assert_eq!(fold_text("日本", 100), b"??");
    assert_eq!(fold_text("abcdef", 4), b"abcd");
    let long = "x".repeat(5000);
    let text = ProtoText::new(&long, "A long one.");
    assert_eq!(text.name.len(), MSG_FIELD_BYTES);

    let pistol = weapon(common(Category::Item, 1), &pistol()).unwrap();
    let described = describe(
        pistol,
        &ProtoText::new("10 mm “Pistol”", "A sturdy side-arm.\nSynthetic."),
    )
    .unwrap();
    assert_eq!(
        described.message_path("English"),
        "text/english/game/pro_item.msg"
    );
    let mut file = MsgFile::from_base(b"{100}{}{Base}\n").unwrap();
    assert!(described.push_messages(&mut file).is_err());
    let mut file = MsgFile::new();
    described.push_messages(&mut file).unwrap();
    let catalog = MsgCatalog::parse_strict(&file.to_bytes()).unwrap();
    assert_eq!(catalog.get(100).unwrap().text, b"10 mm \"Pistol\"");
    assert_eq!(
        catalog.get(101).unwrap().text,
        b"A sturdy side-arm. Synthetic."
    );
    // No message base, no text.
    let mut nameless = common(Category::Item, 2);
    nameless.message_base = proto::NO_MESSAGE;
    let prototype = weapon(nameless, &knife()).unwrap();
    assert!(describe(prototype.clone(), &ProtoText::new("Knife", "")).is_err());
    assert!(
        describe(prototype, &ProtoText::default())
            .unwrap()
            .messages
            .is_empty()
    );
}

/// A small deterministic generator (no randomness at test time).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, range: std::ops::RangeInclusive<i32>) -> i32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let span = (range.end() - range.start() + 1) as u64;
        range.start() + ((self.0 >> 33) % span) as i32
    }
}

#[test]
fn every_accepted_generated_spec_parses_back_equal() {
    let mut rng = Lcg(7);
    let mut accepted = 0;
    for index in 1..=600_u32 {
        let modes = [
            AttackMode::from_code(rng.next(0..=8) as u8).unwrap(),
            AttackMode::from_code(rng.next(0..=8) as u8).unwrap(),
        ];
        let spec = WeaponSpec {
            stats: stats(rng.next(-1..=10), rng.next(0..=30), rng.next(0..=5000)),
            code: WeaponCode::from_code(rng.next(0..=10) as u8).unwrap(),
            damage: [rng.next(0..=20), rng.next(0..=60)],
            damage_type: DamageType::ALL[rng.next(0..=6) as usize],
            modes,
            ap_cost: [rng.next(0..=10), rng.next(0..=10)],
            ranges: [rng.next(0..=50), rng.next(0..=50)],
            min_strength: rng.next(0..=11),
            projectile: None,
            caliber: rng.next(0..=3),
            ammo: None,
            capacity: rng.next(0..=40),
            burst: rng.next(0..=20),
            perk: None,
            critical_failure: rng.next(0..=7),
            big_gun: rng.next(0..=1) == 1,
            two_handed: rng.next(0..=1) == 1,
            sound: b'0' + rng.next(0..=9) as u8,
        };
        let armour = ArmorSpec {
            armor_class: rng.next(-1..=40),
            resistances: std::array::from_fn(|_| rng.next(0..=90)),
            thresholds: std::array::from_fn(|_| rng.next(0..=20)),
            ..leather()
        };
        let mut healer = healer();
        healer.immediate = std::array::from_fn(|_| rng.next(-10..=10));
        healer.addiction_chance = rng.next(0..=110);
        let made = [
            weapon(common(Category::Item, index), &spec),
            armor(common(Category::Item, index), &armour),
            drug(common(Category::Item, index), &healer),
        ];
        for prototype in made.into_iter().flatten() {
            accepted += 1;
            let bytes = LocalWriters.pro(&prototype).unwrap();
            assert_eq!(
                Prototype::parse_for(&bytes, GameProfile::Fallout2).unwrap(),
                prototype
            );
        }
    }
    assert!(accepted > 600, "only {accepted} accepted");
}
