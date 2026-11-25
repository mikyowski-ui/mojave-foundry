//! decode(encode(x)) == x for every typed record, both directly and through
//! a plugin file written by the fixture builder. All data is invented.

use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec, solid_colors, up_normals};
use fnv_esm::*;

fn sub(sig: &[u8; 4], data: &[u8]) -> RawSubrecord {
    RawSubrecord::new(sig, data)
}

/// Heights that `VHGT` stores exactly: multiples of 8 around a base, with
/// neighbour steps inside one signed byte.
fn hilly_heights() -> Vec<f32> {
    (0..LAND_VERTICES)
        .map(|i| {
            let (row, column) = (i / LAND_GRID, i % LAND_GRID);
            1203.5 + 8.0 * ((row * 5 + column * 11) % 60) as f32 - 8.0 * row as f32
        })
        .collect()
}

fn world() -> World {
    World {
        common: Common::new(0x0100_0A00)
            .with_editor_id("TestWorld")
            .with_name("The Test Basin"),
        parent: Some(FormId(0x0000_003C)),
        parent_flags: Some(0x0013),
        encounter_zone: Some(FormId(0x0100_0A01)),
        climate: Some(FormId(0x0100_0A02)),
        water: Some(FormId(0x0100_0A03)),
        lod_water: Some(FormId(0x0100_0A04)),
        lod_water_height: Some(-2048.0),
        land_defaults: Some(LandDefaults {
            land_height: -2048.0,
            water_height: -4096.5,
        }),
        map_image: Some("interface\\worldmap\\test_basin.dds".into()),
        map_data: Some(MapData {
            usable_width: 2048,
            usable_height: 1024,
            north_west: [-12, 9],
            south_east: [14, -11],
        }),
        map_offset: Some(MapOffset {
            scale: 1.25,
            cell_x_offset: -3.0,
            cell_y_offset: 4.5,
        }),
        flags: Some(0x41),
        world_bounds: WorldBounds {
            min: Some([-49152.0, -45056.0]),
            max: Some([57344.0, 36864.0]),
        },
        music: Some(FormId(0x0100_0A05)),
        unknown: vec![sub(b"OFST", &[1, 2, 3, 4, 5, 6, 7, 8])],
    }
}

fn exterior_cell() -> Cell {
    Cell {
        common: Common::new(0x0100_0B00)
            .with_editor_id("TestBasinSquare")
            .with_name("Basin Square"),
        flags: Some(Cell::FLAG_HAS_WATER),
        grid: Some(CellGrid {
            x: -3,
            y: 7,
            hide_land: Some(0b0101),
        }),
        water_height: Some(-12.5),
        water: Some(FormId(0x0100_0B01)),
        lighting: None,
        regions: vec![FormId(0x0100_0B02), FormId(0x0100_0B03)],
        owner: Some(FormId(0x0100_0B04)),
        owner_rank: Some(2),
        encounter_zone: Some(FormId(0x0100_0B05)),
        climate: Some(FormId(0x0100_0B06)),
        image_space: Some(FormId(0x0100_0B07)),
        acoustic_space: Some(FormId(0x0100_0B08)),
        music: Some(FormId(0x0100_0B09)),
        light_template: None,
        unknown: vec![sub(b"XNAM", &[9])],
    }
}

fn interior_cell() -> Cell {
    Cell {
        lighting: Some(CellLighting {
            ambient: [10, 20, 30, 0],
            directional: [40, 50, 60, 0],
            fog_color: [70, 80, 90, 0],
            fog_near: 64.0,
            fog_far: 4096.0,
            directional_rotation_xy: 45,
            directional_rotation_z: -30,
            directional_fade: 0.75,
            fog_clip_distance: 8192.0,
            fog_power: Some(1.5),
        }),
        light_template: Some(FormId(0x0100_0C01)),
        ..Cell::interior(0x0100_0C47, "TestShopInterior")
    }
}

fn land() -> Land {
    let mut normals = up_normals();
    normals[5] = [-64, 32, 100];
    let mut colors = solid_colors([200, 180, 160]);
    colors[1088] = [1, 2, 3];
    Land {
        flags: Some(0x1F),
        normals: Some(normals),
        colors: Some(colors),
        base_layers: vec![
            BaseLayer {
                texture: FormId(0x0100_0D01),
                quadrant: 0,
                layer: -1,
            },
            BaseLayer {
                texture: FormId::NULL,
                quadrant: 3,
                layer: -1,
            },
        ],
        alpha_layers: vec![
            AlphaLayer {
                texture: FormId(0x0100_0D02),
                quadrant: 0,
                layer: 0,
                opacity: vec![
                    AlphaPoint {
                        position: 0,
                        opacity: 1.0,
                    },
                    AlphaPoint {
                        position: 288,
                        opacity: 0.25,
                    },
                ],
            },
            AlphaLayer {
                texture: FormId(0x0100_0D03),
                quadrant: 2,
                layer: 1,
                opacity: Vec::new(),
            },
        ],
        textures: vec![FormId(0x0100_0D01), FormId(0x0100_0D02)],
        unknown: vec![sub(b"ZZZZ", b"kept")],
        ..Land::new(0x0100_0D00).with_heights(&hilly_heights())
    }
}

fn navmesh() -> Navmesh {
    Navmesh {
        cover_triangles: vec![1],
        door_links: vec![NavDoorLink {
            door: FormId(0x0100_0E10),
            triangle: 0,
        }],
        external_links: vec![NavExternalLink {
            unknown: 0xAABB_CCDD,
            navmesh: FormId(0x0100_0E20),
            triangle: 7,
        }],
        unknown: vec![sub(b"NVGD", &[0; 40])],
        ..Navmesh::new(
            0x0100_0E00,
            0x0100_0B00,
            vec![
                [-12288.0, 28672.0, 100.0],
                [-12160.0, 28672.0, 104.0],
                [-12288.0, 28800.0, 96.0],
                [-12160.0, 28800.0, 108.0],
            ],
            vec![
                NavTriangle {
                    vertices: [0, 1, 2],
                    edges: [-1, 1, -1],
                    flags: 0x0000_0001,
                },
                NavTriangle {
                    vertices: [1, 3, 2],
                    edges: [-1, -1, 0],
                    flags: 0x8000_0200,
                },
            ],
        )
    }
}

fn placed_object() -> PlacedRef {
    let mut placed = PlacedRef {
        lock: Some(Lock {
            level: 75,
            key: FormId(0x0100_0F20),
            flags: 0x04,
        }),
        count: Some(3),
        linked_ref: Some(FormId(0x0100_0F21)),
        owner: Some(FormId(0x0100_0F22)),
        owner_rank: Some(-1),
        encounter_zone: Some(FormId(0x0100_0F23)),
        radius: Some(-48.5),
        primitive: Some(Primitive {
            bounds: [64.0, 128.0, 96.0],
            color: [0.25, 0.5, 1.0],
            unknown: 0.75,
            shape: Primitive::SHAPE_BOX,
        }),
        emittance: Some(FormId(0x0100_0F24)),
        unknown: vec![sub(b"XSED", &[7])],
        ..PlacedRef::new(
            PlacedKind::Object,
            0x0100_0F00,
            0x0100_1100,
            [-11000.5, 29000.25, 131.0],
            [0.0, -0.125, 1.5707964],
        )
        .with_scale(1.75)
        .with_teleport(
            0x0100_0F30,
            [16.0, -32.0, 8.0],
            [0.0, 0.0, std::f32::consts::PI],
        )
        .with_enable_parent(0x0100_0F31, true)
        .persistent()
        .initially_disabled()
    };
    placed.common.editor_id = Some("TestShopDoorRef".into());
    placed
}

fn placed_npc() -> PlacedRef {
    PlacedRef::new(
        PlacedKind::Npc,
        0x0100_0F40,
        0x0100_1600,
        [100.0, 200.0, 300.0],
        [0.0, 0.0, 0.5],
    )
    .with_enable_parent(0x0100_0F31, false)
}

fn placed_creature() -> PlacedRef {
    PlacedRef {
        owner: Some(FormId(0x0100_0F22)),
        ..PlacedRef::new(
            PlacedKind::Creature,
            0x0100_0F41,
            0x0100_1700,
            [-5.0, 6.0, -7.0],
            [0.25, 0.5, 0.75],
        )
    }
}

fn statik() -> Static {
    Static {
        common: Common::new(0x0100_1000)
            .with_editor_id("TestWaterTower")
            .with_model("architecture\\test\\watertower01.nif")
            .with_bounds([-120, -130, 0], [120, 130, 900]),
        unknown: vec![sub(b"MODB", &512.0f32.to_le_bytes()), sub(b"BRUS", &[0xFF])],
    }
}

fn door() -> Door {
    Door {
        open_sound: Some(FormId(0x0100_1101)),
        close_sound: Some(FormId(0x0100_1102)),
        loop_sound: Some(FormId(0x0100_1103)),
        flags: Some(0x02),
        common: Common::new(0x0100_1100)
            .with_editor_id("TestShopDoor")
            .with_name("Shop Door")
            .with_model("architecture\\test\\shopdoor.nif")
            .with_script(0x0100_1900u32)
            .with_bounds([-32, -4, 0], [32, 4, 128]),
        unknown: Vec::new(),
    }
}

fn container() -> Container {
    Container {
        data: Some(ContainerData {
            flags: 0x02,
            weight: 12.5,
        }),
        open_sound: Some(FormId(0x0100_1201)),
        close_sound: Some(FormId(0x0100_1202)),
        unknown: vec![sub(b"RNAM", &0x0100_1203u32.to_le_bytes())],
        ..Container::new(
            0x0100_1200,
            "TestFootlocker",
            "Footlocker",
            "clutter\\test\\footlocker.nif",
            vec![
                ContainerItem::new(0x0100_1300, 2),
                ContainerItem::new(0x0100_1400, -1),
            ],
        )
    }
}

fn misc() -> MiscItem {
    MiscItem {
        icon: Some("interface\\icons\\test\\tin_cup.dds".into()),
        small_icon: Some("interface\\icons\\test\\tin_cup_small.dds".into()),
        pickup_sound: Some(FormId(0x0100_1301)),
        drop_sound: Some(FormId(0x0100_1302)),
        ..MiscItem::new(
            0x0100_1300,
            "TestTinCup",
            "Tin Cup",
            "clutter\\test\\tincup.nif",
            3,
            0.5,
        )
    }
}

fn weapon() -> Weapon {
    Weapon {
        common: Common::new(0x0100_1400)
            .with_editor_id("TestPipeRifle")
            .with_name("Pipe Rifle")
            .with_model("weapons\\test\\piperifle.nif"),
        icon: Some("interface\\icons\\test\\piperifle.dds".into()),
        small_icon: None,
        object_effect: Some(FormId(0x0100_1401)),
        ammo: Some(FormId(0x0100_1500)),
        repair_list: Some(FormId(0x0100_1402)),
        equipment_type: Some(5),
        data: Some(WeaponData {
            value: 250,
            health: 400,
            weight: 7.5,
            damage: 22,
            clip_size: 6,
        }),
        critical: Some(WeaponCritical {
            damage: 22,
            chance_multiplier: 1.5,
            flags: 1,
            effect: FormId(0x0100_1403),
        }),
        unknown: vec![sub(b"DNAM", &[0x5A; 204]), sub(b"VNAM", &[1, 0, 0, 0])],
    }
}

fn armor() -> Armor {
    Armor {
        common: Common::new(0x0100_1480)
            .with_editor_id("TestRoadLeathers")
            .with_name("Road Leathers")
            .with_model("armor\\test\\roadleathers_m.nif"),
        biped: Some(BipedData {
            biped_flags: 0x0000_0004,
            general_flags: 0x20,
        }),
        male_world_model: Some("armor\\test\\roadleathers_go.nif".into()),
        female_model: Some("armor\\test\\roadleathers_f.nif".into()),
        female_world_model: Some("armor\\test\\roadleathers_f_go.nif".into()),
        icon: Some("interface\\icons\\test\\roadleathers.dds".into()),
        small_icon: Some("interface\\icons\\test\\roadleathers_s.dds".into()),
        object_effect: None,
        repair_list: Some(FormId(0x0100_1481)),
        biped_model_list: Some(FormId(0x0100_1E20)),
        equipment_type: Some(-1),
        data: Some(ArmorData {
            value: 180,
            health: 150,
            weight: 15.0,
        }),
        stats: Some(ArmorStats {
            armor_rating: 0,
            damage_threshold: Some(6.0),
            flags: 0x0001,
        }),
        unknown: Vec::new(),
    }
}

fn ingestible() -> Ingestible {
    Ingestible {
        common: Common::new(0x0100_14C0)
            .with_editor_id("TestCactusTea")
            .with_name("Cactus Tea")
            .with_model("clutter\\test\\cactustea.nif"),
        icon: Some("interface\\icons\\test\\cactustea.dds".into()),
        small_icon: None,
        equipment_type: Some(10),
        weight: Some(0.25),
        data: Some(IngestibleData {
            value: 8,
            flags: 0x03,
            withdrawal_effect: FormId(0x0100_14C1),
            addiction_chance: 0.05,
            consume_sound: FormId(0x0100_14C2),
        }),
        effects: vec![
            Effect {
                base_effect: FormId(0x0100_14C3),
                data: Some(EffectData {
                    magnitude: 5,
                    area: 0,
                    duration: 30,
                    range: 0,
                    actor_value: 16,
                }),
                conditions: vec![Condition::float(47, Comparison::Less, 50.0).or()],
            },
            Effect {
                base_effect: FormId(0x0100_14C4),
                data: None,
                conditions: Vec::new(),
            },
        ],
        unknown: Vec::new(),
    }
}

fn ammo() -> Ammo {
    Ammo {
        common: Common::new(0x0100_1500)
            .with_editor_id("TestScrapSlug")
            .with_name("Scrap Slug")
            .with_model("weapons\\test\\scrapslug.nif"),
        icon: Some("interface\\icons\\test\\scrapslug.dds".into()),
        small_icon: None,
        data: Some(AmmoData {
            speed: 9000.0,
            flags: 0x01,
            value: 2,
            clip_rounds: 6,
        }),
        data2: Some(AmmoData2 {
            projectiles_per_shot: 1,
            projectile: FormId(0x0100_1501),
            weight: 0.04,
            consumed_ammo: FormId(0x0100_1502),
            consumed_percentage: 0.5,
        }),
        short_name: Some("Slug".into()),
        abbreviation: Some("SL".into()),
        unknown: Vec::new(),
    }
}

fn npc() -> Npc {
    Npc {
        common: Common::new(0x0100_1600)
            .with_editor_id("TestShopkeeper")
            .with_name("Marla Venn")
            .with_script(0x0100_1900u32),
        base: Some(ActorBase {
            flags: 0x0000_0011,
            fatigue: 60,
            barter_gold: 400,
            level: 4,
            calc_min: 1,
            calc_max: 10,
            speed_multiplier: 100,
            karma: 250.0,
            disposition_base: 35,
            template_flags: 0x0005,
        }),
        factions: vec![
            FactionRank {
                faction: FormId(0x0100_1601),
                rank: 2,
            },
            FactionRank {
                faction: FormId(0x0100_1602),
                rank: 0,
            },
        ],
        death_item: Some(FormId(0x0100_1603)),
        ai: Some(AiData {
            aggression: 1,
            confidence: 3,
            energy_level: 50,
            responsibility: 70,
            mood: 5,
            services: 0x0000_0403,
            trained_skill: 4,
            trainer_level: 30,
            assistance: 2,
            aggro_radius_behaviour: 1,
            aggro_radius: 500,
        }),
        voice: Some(FormId(0x0100_1604)),
        template: Some(FormId(0x0100_1605)),
        race: Some(FormId(0x0100_1606)),
        class: Some(FormId(0x0100_1607)),
        actor_effects: vec![FormId(0x0100_1608)],
        inventory: vec![
            ContainerItem::new(0x0100_1300, 1),
            ContainerItem::new(0x0100_1400, 1),
        ],
        packages: vec![FormId(0x0100_1609), FormId(0x0100_160A)],
        stats: Some(NpcStats {
            base_health: 85,
            attributes: [5, 6, 4, 7, 5, 6, 3],
        }),
        skills: Some(NpcSkills {
            values: [20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85],
            offsets: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
        }),
        head_parts: vec![FormId(0x0100_160B)],
        hair: Some(FormId(0x0100_160C)),
        hair_length: Some(0.6),
        eyes: Some(FormId(0x0100_160D)),
        hair_color: Some(Rgb {
            r: 90,
            g: 60,
            b: 30,
        }),
        combat_style: Some(FormId(0x0100_160E)),
        facegen_geometry: Some((0..200u8).collect()),
        facegen_asymmetry: Some((0..120u8).map(|b| b ^ 0x55).collect()),
        facegen_texture: Some((0..200u8).rev().collect()),
        height: Some(1.02),
        weight: Some(48.0),
        unknown: vec![sub(b"QQQQ", &[7; 20])],
    }
}

fn creature() -> Creature {
    Creature {
        model_list: vec!["dustrat.nif".into(), "dustrat_eyes.nif".into()],
        animations: vec!["idle_sniff.kf".into()],
        factions: vec![FactionRank {
            faction: FormId(0x0100_1701),
            rank: 0,
        }],
        death_item: Some(FormId(0x0100_1702)),
        voice: Some(FormId(0x0100_1703)),
        template: None,
        actor_effects: vec![FormId(0x0100_1704)],
        inventory: vec![ContainerItem::new(0x0100_1300, 1)],
        packages: vec![FormId(0x0100_1705)],
        data: Some(CreatureData {
            creature_type: 1,
            combat_skill: 40,
            magic_skill: 0,
            stealth_skill: 25,
            health: 35,
            damage: 6,
            attributes: [4, 5, 4, 1, 2, 7, 5],
        }),
        combat_style: Some(FormId(0x0100_1706)),
        base_scale: Some(0.8),
        unknown: vec![sub(b"TNAM", &90.0f32.to_le_bytes())],
        ..Creature::new(
            0x0100_1700,
            "TestDustRat",
            "Dust Rat",
            "creatures\\test\\dustrat\\skeleton.nif",
        )
    }
}

fn stage_script() -> ScriptBody {
    ScriptBody {
        header: Some(ScriptHeader {
            ref_count: 2,
            compiled_size: 6,
            variable_count: 1,
            script_type: 0,
            flags: 1,
        }),
        compiled: Some(vec![0x1D, 0x00, 0x02, 0x00, 0x10, 0x00]),
        source: Some("set tinCupCount to 3\r\nTestShopDoorRef.unlock".into()),
        locals: vec![LocalVariable {
            index: 1,
            flags: 1,
            name: Some("tinCupCount".into()),
        }],
        references: vec![
            ScriptReference::Form(FormId(0x0100_0F00)),
            ScriptReference::Local(1),
        ],
    }
}

fn quest() -> Quest {
    Quest {
        icon: Some("interface\\icons\\test\\quest_cups.dds".into()),
        data: Some(QuestData {
            flags: 0x01,
            priority: 55,
            delay: Some(2.5),
        }),
        conditions: vec![
            Condition::float(74, Comparison::GreaterOrEqual, 2.0).with_params(0x0100_1601, 0),
            Condition::global(58, Comparison::Equal, 0x0100_1810u32),
        ],
        stages: vec![
            QuestStage {
                index: 10,
                entries: vec![QuestLogEntry {
                    flags: Some(0),
                    conditions: vec![Condition::float(72, Comparison::NotEqual, 1.0)],
                    text: Some("Marla wants three tin cups for the shop.".into()),
                    script: stage_script(),
                    next_quest: None,
                }],
            },
            QuestStage {
                index: 20,
                entries: vec![
                    QuestLogEntry::new("I found the cups."),
                    QuestLogEntry {
                        flags: Some(0x01),
                        conditions: Vec::new(),
                        text: Some("Marla paid me for the cups.".into()),
                        script: ScriptBody::from_source("RewardXP 50"),
                        next_quest: Some(FormId(0x0100_1820)),
                    },
                ],
            },
            QuestStage {
                index: 30,
                entries: Vec::new(),
            },
        ],
        objectives: vec![
            QuestObjective {
                index: 10,
                description: Some("Find three tin cups.".into()),
                targets: vec![
                    QuestTarget {
                        target: FormId(0x0100_0F00),
                        flags: 1,
                        conditions: vec![Condition::float(58, Comparison::Less, 20.0)],
                    },
                    QuestTarget {
                        target: FormId(0x0100_0F40),
                        flags: 0,
                        conditions: Vec::new(),
                    },
                ],
            },
            QuestObjective {
                index: 20,
                description: Some("Return to Marla.".into()),
                targets: Vec::new(),
            },
        ],
        unknown: Vec::new(),
        ..Quest::new(0x0100_1800, "TestCupsQuest", "Three Tin Cups")
    }
}

fn topic() -> Topic {
    Topic {
        removed_quests: vec![FormId(0x0100_1820)],
        priority: Some(50.0),
        dumb_response: Some("Cups?".into()),
        data: Some(TopicData {
            topic_type: 0,
            flags: Some(0x01),
        }),
        unknown: vec![sub(b"INFC", &0x0100_1A01u32.to_le_bytes())],
        ..Topic::new(
            0x0100_1A00,
            "TestCupsTopic",
            "About those cups...",
            0x0100_1800u32,
        )
    }
}

fn info() -> Info {
    Info {
        common: Common::new(0x0100_1A01),
        data: Some(InfoData {
            info_type: 0,
            next_speaker: 1,
            flags: Some(0x0105),
            short: false,
        }),
        quest: Some(FormId(0x0100_1800)),
        topic: Some(FormId(0x0100_1A00)),
        previous_info: Some(FormId(0x0100_1A02)),
        add_topics: vec![FormId(0x0100_1A10)],
        responses: vec![
            Response {
                script_notes: Some("tired, friendly".into()),
                edits: Some("v2".into()),
                speaker_animation: Some(FormId(0x0100_1A20)),
                listener_animation: Some(FormId(0x0100_1A21)),
                ..Response::new(1, "Three cups. Tin ones. That is the whole job.")
                    .with_emotion(5, 40)
            },
            Response::new(2, "Bring them here and I will pay you."),
        ],
        conditions: vec![
            Condition::float(72, Comparison::Equal, 1.0)
                .with_params(0x0100_1600, 0)
                .or(),
            Condition {
                run_on: 2,
                reference: FormId(0x0100_0F40),
                ..Condition::float(58, Comparison::LessOrEqual, 10.0).with_params(0x0100_1800, 0)
            },
        ],
        choices: vec![FormId(0x0100_1A30), FormId(0x0100_1A31)],
        link_from: vec![FormId(0x0100_1A32)],
        follow_up: vec![FormId(0x0100_1A33)],
        result_begin: stage_script(),
        result_end: ScriptBody::from_source("SetStage TestCupsQuest 10"),
        prompt: Some("What do you need?".into()),
        speaker: Some(FormId(0x0100_1600)),
        speech_challenge: Some(3),
        knam: Some(FormId(0x0100_1A40)),
        unknown: vec![sub(b"QQQQ", &0x0100_1A41u32.to_le_bytes())],
    }
}

fn script() -> Script {
    Script {
        common: Common::new(0x0100_1900).with_editor_id("TestShopDoorScript"),
        body: ScriptBody {
            header: Some(ScriptHeader {
                ref_count: 1,
                compiled_size: 4,
                variable_count: 2,
                script_type: 0,
                flags: 1,
            }),
            compiled: Some(vec![0x1D, 0x00, 0x00, 0x00]),
            source: Some(
                "scn TestShopDoorScript\r\n\r\nshort opened\r\nfloat timer\r\n\r\nbegin OnActivate\r\n\tset opened to 1\r\n\tActivate\r\nend\r\n"
                    .into(),
            ),
            locals: vec![
                LocalVariable {
                    index: 1,
                    flags: 1,
                    name: Some("opened".into()),
                },
                LocalVariable {
                    index: 2,
                    flags: 0,
                    name: Some("timer".into()),
                },
            ],
            references: vec![ScriptReference::Form(FormId(0x0000_0014))],
        },
        unknown: Vec::new(),
    }
}

fn land_texture() -> LandTexture {
    LandTexture {
        icon: Some("landscape\\test\\packed_dirt_icon.dds".into()),
        small_icon: None,
        havok: Some(LandHavok {
            material: 2,
            friction: 30,
            restitution: 30,
        }),
        specular_exponent: Some(30),
        grasses: vec![FormId(0x0100_1B10), FormId(0x0100_1B11)],
        ..LandTexture::new(0x0100_0D01, "TestPackedDirt", 0x0100_1B00u32)
    }
}

fn texture_set() -> TextureSet {
    let mut set = TextureSet::new(
        0x0100_1B00,
        "TestPackedDirtSet",
        "landscape\\test\\packed_dirt.dds",
    );
    set.textures[1] = Some("landscape\\test\\packed_dirt_n.dds".into());
    set.textures[5] = Some("landscape\\test\\packed_dirt_e.dds".into());
    set.flags = Some(0x0001);
    set.common = set.common.with_bounds([0, 0, 0], [0, 0, 0]);
    set.unknown = vec![sub(b"DODT", &[3; 36])];
    set
}

fn race() -> Race {
    let parts = |prefix: &str| RaceParts {
        male: vec![
            RacePart::new(
                0,
                Some(&format!("characters\\test\\{prefix}m.nif")),
                Some(&format!("characters\\test\\{prefix}m.dds")),
            ),
            RacePart::new(1, None, Some("characters\\test\\ears.dds")),
        ],
        female: vec![RacePart::new(
            0,
            Some(&format!("characters\\test\\{prefix}f.nif")),
            None,
        )],
    };
    Race {
        common: Common::new(0x0100_1E00)
            .with_editor_id("TestDesertFolk")
            .with_name("Desert Folk"),
        description: Some(String::new()),
        data: Some(RaceData {
            skill_boosts: [[32, 5], [33, 0], [0, 0], [0, 0], [0, 0], [0, 0], [0, 0]],
            height: [1.0, 0.98],
            weight: [1.0, 0.9],
            flags: 1,
        }),
        default_hair: Some([FormId(0x0100_1E01), FormId(0x0100_1E02)]),
        head: Some(parts("head")),
        body: Some(parts("body")),
        hairs: vec![FormId(0x0100_1E01), FormId(0x0100_1E02)],
        eyes: vec![FormId(0x0100_1E03)],
        unknown: vec![sub(b"VTCK", &[1, 0, 0, 0, 2, 0, 0, 0])],
    }
}

fn head_part() -> HeadPart {
    HeadPart {
        flags: Some(1),
        extra_parts: vec![FormId(0x0100_1E05)],
        ..HeadPart::new(0x0100_1E04, "TestBeard", "characters\\test\\beard.nif")
    }
}

fn hair() -> Hair {
    Hair::new(
        0x0100_1E01,
        "TestHairShort",
        "characters\\test\\hairshort.nif",
        "characters\\test\\hairshort.dds",
    )
}

fn eyes() -> Eyes {
    Eyes {
        common: Common::new(0x0100_1E03)
            .with_editor_id("TestEyesBrown")
            .with_name("Brown"),
        ..Eyes::new(
            0x0100_1E03,
            "TestEyesBrown",
            "characters\\test\\eyebrown.dds",
        )
    }
}

fn armor_addon() -> ArmorAddon {
    ArmorAddon {
        female_model: Some("armor\\test\\glove_f.nif".into()),
        icon: Some("interface\\icons\\test\\glove.dds".into()),
        equipment_type: Some(9),
        data: Some(ArmorData {
            value: 0,
            health: 0,
            weight: 0.0,
        }),
        stats: Some(ArmorStats {
            armor_rating: 0,
            damage_threshold: None,
            flags: 0,
        }),
        ..ArmorAddon::new(
            0x0100_1E21,
            "TestGloveLeft",
            BipedData::SLOT_LEFT_HAND,
            "armor\\test\\glove_m.nif",
        )
    }
}

fn form_list() -> FormList {
    FormList::new(0x0100_1E20, "TestGloveList", vec![FormId(0x0100_1E21)])
}

fn leveled(kind: LeveledKind, form: u32) -> LeveledList {
    let mut list = LeveledList::new(
        kind,
        form,
        "TestLeveled",
        vec![
            LeveledEntry::new(1, 0x0100_1600u32, 1),
            LeveledEntry {
                extra: Some(LeveledExtra {
                    owner: FormId::NULL,
                    value: 0,
                    condition: 0.5,
                }),
                ..LeveledEntry::new(5, 0x0100_1480u32, 2)
            },
        ],
    );
    list.chance_none = Some(25);
    list.flags = Some(LeveledList::FLAG_USE_ALL);
    if kind == LeveledKind::Item {
        list.global = Some(FormId(0x0100_1E30));
    }
    list.common = list.common.with_bounds([0, 0, 0], [0, 0, 0]);
    list
}

/// One of every typed record, in decoder-table order of types.
fn samples() -> Vec<Record> {
    vec![
        world().into(),
        exterior_cell().into(),
        interior_cell().into(),
        land().into(),
        navmesh().into(),
        placed_object().into(),
        placed_npc().into(),
        placed_creature().into(),
        statik().into(),
        door().into(),
        container().into(),
        misc().into(),
        weapon().into(),
        armor().into(),
        ingestible().into(),
        ammo().into(),
        npc().into(),
        creature().into(),
        quest().into(),
        topic().into(),
        info().into(),
        script().into(),
        land_texture().into(),
        texture_set().into(),
        race().into(),
        head_part().into(),
        hair().into(),
        eyes().into(),
        armor_addon().into(),
        form_list().into(),
        leveled(LeveledKind::Npc, 0x0100_1E40).into(),
        leveled(LeveledKind::Creature, 0x0100_1E41).into(),
        leveled(LeveledKind::Item, 0x0100_1E42).into(),
    ]
}

#[test]
fn samples_cover_every_row_of_the_decoder_table() {
    let samples = samples();
    for entry in DECODERS {
        assert!(
            samples.iter().any(|record| record.sig() == entry.sig),
            "no sample for {}",
            entry.sig
        );
        assert_eq!(
            decoder_for(entry.sig).map(|e| e.type_name),
            Some(entry.type_name)
        );
    }
    assert_eq!(DECODERS.len(), 32);
    assert!(decoder_for(Sig::new(b"SOUN")).is_none());
}

#[test]
fn every_typed_record_round_trips_through_its_subrecords() {
    for record in samples() {
        let raw = record.to_raw();
        assert_eq!(raw.sig, record.sig());
        assert_eq!(raw.meta, *record.meta());
        let decoded = raw.decode();
        assert_eq!(decoded, record, "{} did not round-trip", record.sig());
        assert!(
            !matches!(decoded, Record::Unknown(_)),
            "{} has no decoder",
            record.sig()
        );
        // Encoding a decoded record is stable.
        assert_eq!(decoded.to_raw(), raw);
    }
}

#[test]
fn every_typed_record_round_trips_compressed() {
    for record in samples() {
        let mut raw = record.to_raw();
        raw.meta.flags.0 |= RecordFlags::COMPRESSED;
        let mut builder = PluginBuilder::new();
        builder.add_raw(raw.clone());
        let bytes = builder.build();
        let plugin = Plugin::parse(&bytes).expect("plugin");
        let stored = plugin
            .walk()
            .next_record()
            .expect("walk")
            .expect("one record");
        assert!(stored.flags().is_compressed());
        assert!(stored.data().expect("inflate").was_compressed());
        assert_eq!(stored.to_raw().expect("raw"), raw);
        let decoded = stored.decode().expect("decode");
        assert!(decoded.meta().flags.is_compressed());
        assert_eq!(decoded.to_raw(), raw);
    }
}

#[test]
fn every_typed_record_round_trips_through_a_plugin_file() {
    let mut builder = PluginBuilder::new();
    builder
        .master_file(true)
        .author("fixture")
        .master("Base.esm");
    // Base objects, actors and the rest: one top-level group per type.
    builder
        .add(texture_set())
        .add(land_texture())
        .add(statik())
        .add(door())
        .add(container())
        .add(misc())
        .add(weapon())
        .add(armor())
        .add(ingestible())
        .add(ammo())
        .add(npc())
        .add(creature())
        .add(script())
        .add(quest())
        .add(race())
        .add(head_part())
        .add(hair())
        .add(eyes())
        .add(armor_addon())
        .add(form_list())
        .add(leveled(LeveledKind::Npc, 0x0100_1E40))
        .add(leveled(LeveledKind::Creature, 0x0100_1E41))
        .add(leveled(LeveledKind::Item, 0x0100_1E42));
    builder.world(
        WorldSpec::new(world()).with_cell(
            CellSpec::new(exterior_cell())
                .with_land(land())
                .with_navmesh(navmesh())
                .with_persistent(placed_object())
                .with_temporary(placed_npc())
                .with_visible_distant(placed_creature()),
        ),
    );
    builder.interior(CellSpec::new(interior_cell()));
    builder.topic(topic(), vec![info()]);
    let bytes = builder.build();

    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut decoded = Vec::new();
    while let Some(record) = walker.next_record().expect("walk") {
        decoded.push(record.decode().expect("decode"));
    }
    let mut expected = samples();
    assert_eq!(decoded.len(), expected.len());
    // The file order differs from the sample order; compare as sets keyed
    // by signature and form id.
    let key = |r: &Record| (r.sig(), r.form_id());
    decoded.sort_by_key(key);
    expected.sort_by_key(key);
    assert_eq!(decoded, expected);
}

#[test]
fn script_source_text_is_extracted_wherever_a_script_is_embedded() {
    let Record::Script(script) = script().to_raw().decode() else {
        panic!("SCPT decodes to a script");
    };
    assert!(
        script
            .body
            .source
            .as_deref()
            .is_some_and(|s| s.starts_with("scn TestShopDoorScript"))
    );

    let Record::Quest(quest) = quest().to_raw().decode() else {
        panic!("QUST decodes to a quest");
    };
    assert_eq!(
        quest.stages[0].entries[0].script.source.as_deref(),
        Some("set tinCupCount to 3\r\nTestShopDoorRef.unlock")
    );
    assert_eq!(
        quest.stages[1].entries[1].script.source.as_deref(),
        Some("RewardXP 50")
    );
    assert!(quest.stages[1].entries[0].script.is_empty());

    let Record::Info(info) = info().to_raw().decode() else {
        panic!("INFO decodes to an info");
    };
    assert!(info.result_begin.source.is_some());
    assert_eq!(
        info.result_end.source.as_deref(),
        Some("SetStage TestCupsQuest 10")
    );

    // Source text is not NUL-terminated: every byte of SCTX is text.
    let raw = RawRecord::new(b"SCPT", 0x77u32).with(b"SCTX", &b"short a\nshort b"[..]);
    let Record::Script(bare) = raw.decode() else {
        panic!("SCPT decodes to a script");
    };
    assert_eq!(bare.body.source.as_deref(), Some("short a\nshort b"));
    assert_eq!(bare.body.header, None);
}

#[test]
fn only_the_end_script_is_written_after_the_marker() {
    let only_end = Info {
        result_end: ScriptBody::from_source("ShowMessage TestDone"),
        ..Info::new(0x10u32, 0x20u32, vec![Response::new(1, "Done.")])
    };
    let raw = only_end.to_raw();
    let sigs: Vec<String> = raw.subrecords.iter().map(|s| s.sig.to_string()).collect();
    let next = sigs.iter().position(|s| s == "NEXT").expect("NEXT marker");
    assert_eq!(&sigs[next + 1..], ["SCHR", "SCTX"]);
    assert_eq!(raw.decode(), Record::Info(only_end));
}

#[test]
fn common_fields_and_flags_are_reported_for_every_type() {
    let door = Record::from(door());
    let common = door.common().expect("typed record");
    assert_eq!(door.form_id(), FormId(0x0100_1100));
    assert_eq!(door.editor_id(), Some("TestShopDoor"));
    assert_eq!(common.full_name.as_deref(), Some("Shop Door"));
    assert_eq!(
        common.model.as_deref(),
        Some("architecture\\test\\shopdoor.nif")
    );
    assert_eq!(common.script, Some(FormId(0x0100_1900)));
    assert_eq!(
        common.bounds,
        Some(ObjectBounds {
            min: [-32, -4, 0],
            max: [32, 4, 128]
        })
    );

    let placed = placed_object();
    assert!(placed.common.flags().is_persistent());
    assert!(placed.common.flags().is_initially_disabled());
    assert!(!placed.common.flags().is_deleted());
    assert!(!placed.common.flags().is_compressed());

    let mut raw = statik().to_raw();
    raw.meta.flags.0 |= RecordFlags::DELETED | RecordFlags::VISIBLE_WHEN_DISTANT;
    raw.meta.revision = 0x1234_5678;
    raw.meta.form_version = 15;
    raw.meta.unknown = 2;
    let decoded = raw.decode();
    assert!(decoded.meta().flags.is_deleted());
    assert!(decoded.meta().flags.is_visible_when_distant());
    assert_eq!(decoded.meta().revision, 0x1234_5678);
    assert_eq!(decoded.meta().form_version, 15);
    assert_eq!(decoded.to_raw(), raw);
}

#[test]
fn placed_reference_defaults_and_short_forms() {
    // No XSCL: scale 1.0. A 28-byte XTEL and a 12-byte XLOC are the short
    // forms of the same structs.
    let mut xtel = 0x55u32.to_le_bytes().to_vec();
    for v in [1.0f32, 2.0, 3.0, 0.0, 0.0, 0.5] {
        xtel.extend_from_slice(&v.to_le_bytes());
    }
    let mut xloc = vec![50, 0, 0, 0];
    xloc.extend_from_slice(&0x66u32.to_le_bytes());
    xloc.extend_from_slice(&[4, 0, 0, 0]);
    let raw = RawRecord::new(b"REFR", 0x99u32)
        .with(b"NAME", 0x44u32.to_le_bytes())
        .with(b"XTEL", xtel)
        .with(b"XLOC", xloc);
    let Record::Placed(placed) = raw.decode() else {
        panic!("REFR decodes to a placed reference");
    };
    assert_eq!(placed.kind, PlacedKind::Object);
    assert_eq!(placed.base, Some(FormId(0x44)));
    assert_eq!(placed.scale, 1.0);
    assert_eq!(placed.placement, None);
    let teleport = placed.teleport.expect("teleport");
    assert_eq!(teleport.door, FormId(0x55));
    assert_eq!(teleport.position, [1.0, 2.0, 3.0]);
    assert_eq!(teleport.rotation, [0.0, 0.0, 0.5]);
    assert_eq!(teleport.flags, 0);
    assert_eq!(
        placed.lock,
        Some(Lock {
            level: 50,
            key: FormId(0x66),
            flags: 4
        })
    );
    assert!(placed.unknown.is_empty());

    for (sig, kind) in [(b"ACHR", PlacedKind::Npc), (b"ACRE", PlacedKind::Creature)] {
        let Record::Placed(placed) = RawRecord::new(sig, 1u32).decode() else {
            panic!("placed reference");
        };
        assert_eq!(placed.kind, kind);
        assert_eq!(placed.to_raw().sig, Sig::new(sig));
    }
}

#[test]
fn land_layers_and_grids_decode_as_documented() {
    let Record::Land(land) = land().to_raw().decode() else {
        panic!("LAND decodes to a land");
    };
    let heights = hilly_heights();
    assert_eq!(land.heights.as_deref(), Some(heights.as_slice()));
    // Row 0 is the south edge, column 0 the west edge.
    assert_eq!(land.height(0, 0), Some(1203.5));
    assert_eq!(land.height(0, 1), Some(1203.5 + 88.0));
    assert_eq!(land.height(1, 0), Some(1203.5 + 40.0 - 8.0));
    assert_eq!(land.height(32, 32), Some(heights[1088]));
    assert_eq!(land.height(33, 0), None);
    assert_eq!(
        land.normals.as_ref().map(|n| (n.len(), n[5])),
        Some((1089, [-64, 32, 100]))
    );
    assert_eq!(
        land.colors.as_ref().map(|c| (c.len(), c[1088])),
        Some((1089, [1, 2, 3]))
    );
    assert_eq!(land.base_layers.len(), 2);
    assert_eq!(land.alpha_layers[0].opacity.len(), 2);
    assert_eq!(land.alpha_layers[0].opacity[1].position, 288);
    assert!(land.alpha_layers[1].opacity.is_empty());
}

#[test]
fn typed_records_serialize() {
    for record in samples() {
        let json = serde_json::to_string(&record).expect("serialize");
        let back: Record = serde_json::from_str(&json).expect("deserialize");
        // JSON prints the shortest decimal that reads back to the same f32.
        assert_eq!(back, record, "{} changed through JSON", record.sig());
        // Same value, same text: nothing unordered reaches the output.
        assert_eq!(serde_json::to_string(&back).expect("serialize"), json);
    }
    let json = serde_json::to_string(&Sig::new(b"NPC_")).expect("serialize");
    assert_eq!(json, "\"NPC_\"");
    assert!(serde_json::from_str::<Sig>("\"TOOLONG\"").is_err());
    assert!(serde_json::from_str::<Sig>("\"ABC\"").is_err());
}
