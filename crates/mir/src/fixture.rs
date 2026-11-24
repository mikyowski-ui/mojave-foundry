//! A small synthetic install to run the whole pipeline on.
//!
//! [`mini_town`] writes a `Data` directory with one master file and two
//! archives, built with the parser crates' fixture builders.
//! [`fallout3_town`] writes a smaller Fallout 3 one (`HEDR` 0.94): a child
//! worldspace on its parent's land, the older subrecord sizes, a terminal,
//! a note, a package and references pointed at in every way. Everything in
//! it is invented. It is small enough to build and extract in well under a
//! second, and its bytes depend on nothing but this code.

use std::f32::consts::FRAC_PI_4;
use std::fs;
use std::path::{Path, PathBuf};

use fnv_bsa::fixture::BsaBuilder;
use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec, solid_colors};
use fnv_esm::{
    ActorBase, AlphaLayer, AlphaPoint, Armor, ArmorAddon, ArmorData, BaseLayer, BipedData, Cell,
    Comparison, Condition, Container, ContainerItem, Creature, CreatureData, Door, Eyes, FormId,
    FormList, Hair, Info, LAND_GRID, Land, LandTexture, LeveledEntry, LeveledKind, LeveledList,
    MiscItem, NavDoorLink, NavTriangle, Navmesh, Npc, NpcStats, PlacedKind, PlacedRef, Primitive,
    Quest, QuestLogEntry, QuestObjective, QuestStage, Race, RaceData, RacePart, RaceParts,
    RawRecord, RecordFlags, Response, Rgb, Script, ScriptBody, Static, TextureSet, Topic, World,
};
use nif::fixture::{
    KfDesc, KfTrack, NifBuilder, NodeDesc, box_mesh, box_shape, building_scene, linear_rotations,
    linear_translations, quad_shape, skeleton_bones, skinned_body, texture_slots, unset_transform,
    walk_kf,
};
use nif::{Block, BsShaderPpLighting, BsShaderTextureSet, NiTransformData, QuatTransform};

use crate::extract::ExtractOptions;
use crate::model::{RawStore, Slice};
use crate::paths::FormKey;

mod fallout3;

pub use fallout3::{
    CHILD_WORLDSPACE, FALLOUT3_PLUGIN, Fallout3Town, PARENT_WORLDSPACE, fallout3_town,
};

/// The plugin's file name.
pub const PLUGIN: &str = "MiniTown.esm";
/// The worldspace's editor id.
pub const WORLDSPACE: &str = "MiniTownWorld";
/// The interior's editor id.
pub const INTERIOR: &str = "MTHouseInterior";
/// The stand-in model of the trigger activator: an ordinary archived mesh
/// (the fence's), which a trigger volume must not draw.
pub const TRIGGER_STAND_IN: &str = "meshes\\minitown\\fence.nif";
/// A model path that no archive has.
pub const MISSING_MESH: &str = "meshes\\minitown\\lostcrate.nif";
/// A texture path whose archived bytes are not a DDS file.
pub const CORRUPT_TEXTURE: &str = "textures\\minitown\\sign.dds";
/// A creature body model that no archive has.
pub const MISSING_PART: &str = "meshes\\minitown\\creatures\\dog\\dogtail.nif";
/// The sheriff's one race body part, the male right hand: the skinned
/// shape of `nif::fixture::skinned_body` (bound to `nif::fixture::SKIN_BONES`
/// of the human skeleton, vertices `nif::fixture::BODY_VERTICES`), with a
/// texture set added so that it renders like any other part.
pub const SKINNED_PART: &str = "meshes\\minitown\\characters\\righthandmale.nif";
/// Length in seconds of the standing idle ([`stand_kf`]).
pub const STAND_DURATION: f32 = 2.0;

/// What [`mini_town`] wrote, and the forms a test wants to look at.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniTown {
    /// The `Data` directory.
    pub data_dir: PathBuf,
    /// The plugin list: [`PLUGIN`].
    pub plugins: Vec<String>,
    /// The slice covering the whole town: 3 × 3 cells, `x` and `y` from −1
    /// to 1, interior depth 1.
    pub slice: Slice,
    /// The house's outside door: a teleport into the interior, filed in the
    /// worldspace's persistent cell; it stands in cell (0, 0).
    pub exterior_door: FormKey,
    /// The door inside the house, leading back out.
    pub interior_door: FormKey,
    /// The interior cell.
    pub interior_cell: FormKey,
    /// The house: a multi-part building several floor squares wide.
    pub house: FormKey,
    /// A fence rotated 45° about Z.
    pub rotated_fence: FormKey,
    /// A rock scaled by 2.
    pub scaled_rock: FormKey,
    /// The crate, a container with two items.
    pub crate_ref: FormKey,
    /// A bottle whose enable parent is the rock, opposite.
    pub bottle: FormKey,
    /// A placed misc item whose model is missing ([`MISSING_MESH`]).
    pub lost_crate: FormKey,
    /// The sheriff: a persistent NPC reference in cell (1, 0).
    pub sheriff_ref: FormKey,
    /// The sheriff's base NPC: male, wears a duster (upper body, with a
    /// left glove from its biped model list), a hat over the hair, and a
    /// vest that loses the upper body slot to the duster.
    pub sheriff: FormKey,
    /// The deputy, a female NPC placed in the interior. It takes its
    /// traits, model and name from a template that is a leveled NPC list,
    /// whose first lowest-level entry is the farmer; its own inventory
    /// holds a leveled item list that resolves to goggles (no female
    /// model, so the male one is worn), and an item outside the plugin.
    pub deputy: FormKey,
    /// The deputy's reference, in the interior.
    pub deputy_ref: FormKey,
    /// The farmer: the deputy's template, never placed.
    pub farmer: FormKey,
    /// The leveled NPC list the deputy names as its template.
    pub settlers: FormKey,
    /// The leveled item list in the deputy's inventory.
    pub outfit_list: FormKey,
    /// The race, with head and body parts for both sexes.
    pub race: FormKey,
    /// A dog: a creature whose body models sit in its skeleton's folder,
    /// one of them missing ([`MISSING_PART`]).
    pub dog: FormKey,
    /// The dog's reference, in the interior.
    pub dog_ref: FormKey,
    /// The door script.
    pub script: FormKey,
    /// The quest.
    pub quest: FormKey,
    /// The dialogue topic.
    pub topic: FormKey,
    /// The navmesh of cell (0, 0).
    pub navmesh: FormKey,
    /// A trigger volume in cell (−1, 1): a reference to an activator whose
    /// model is a stand-in ([`TRIGGER_STAND_IN`]), with a box primitive
    /// (`XPRM`). Its document has `trigger_volume` set; no other reference
    /// has.
    pub trigger_volume: FormKey,
    /// The trigger volume's base, an activator (`ACTI`).
    pub trigger_base: FormKey,
    /// The cell (0, 0).
    pub centre_cell: FormKey,
    /// The actor a character test animates: the sheriff ([`Self::sheriff`],
    /// placed outdoors by [`Self::sheriff_ref`]). Its document has a
    /// skeleton hash (the human skeleton, whose bones are those of
    /// `nif::fixture::skeleton_bones`), both animation hashes (stand:
    /// [`stand_kf`]; walk: `nif::fixture::walk_kf`) and the skinned body
    /// part [`SKINNED_PART`]. The deputy has the same skeleton and
    /// animations, but no skinned part.
    pub animated_actor: FormKey,
}

impl MiniTown {
    /// Extract options for this install.
    pub fn options(&self, out: &Path, jobs: usize) -> ExtractOptions {
        ExtractOptions {
            data_dir: self.data_dir.clone(),
            plugins: self.plugins.clone(),
            out: out.to_path_buf(),
            slice: self.slice.clone(),
            jobs,
            raw_store: RawStore::All,
        }
    }
}

fn key(id: u32) -> FormKey {
    FormKey::new(PLUGIN, id)
}

/// Terrain height at a source position: a gentle slope up to the
/// north-east and one steep ridge running east-west at `y = 1536`.
pub fn terrain_height(x: f32, y: f32) -> f32 {
    let slope = 0.04 * x + 0.02 * y;
    let ridge = 600.0 * (1.0 - (y - 1536.0).abs() / 256.0).max(0.0);
    1000.0 + slope + ridge
}

fn land_heights(cx: i32, cy: i32) -> Vec<f32> {
    let mut heights = Vec::with_capacity(LAND_GRID * LAND_GRID);
    for row in 0..LAND_GRID {
        for column in 0..LAND_GRID {
            let x = cx as f32 * 4096.0 + column as f32 * 128.0;
            let y = cy as f32 * 4096.0 + row as f32 * 128.0;
            heights.push(terrain_height(x, y));
        }
    }
    heights
}

/// A `w × h` RGBA test pattern in one hue.
fn pattern(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let shade = if (x / 4 + y / 4) % 2 == 0 { 255 } else { 160 };
            let scale = |c: u8| (u16::from(c) * shade / 255) as u8;
            out.extend_from_slice(&[scale(rgb[0]), scale(rgb[1]), scale(rgb[2]), 255]);
        }
    }
    out
}

fn bc1(rgb: [u8; 3]) -> Vec<u8> {
    dds::fixture::encode_bc1(16, 16, &pattern(16, 16, rgb))
}

fn flat_normal_map() -> Vec<u8> {
    let rgba: Vec<u8> = std::iter::repeat_n([128u8, 128, 255, 255], 64)
        .flatten()
        .collect();
    dds::fixture::encode_uncompressed(8, 8, &rgba, dds::fixture::UncompressedLayout::Bgra8)
}

/// A navmesh over a 4 × 4 vertex grid, with links across shared edges.
fn navmesh(form: u32, cell: u32, door: u32) -> Navmesh {
    let mut vertices = Vec::new();
    for j in 0..4 {
        for i in 0..4 {
            let x = 1024.0 + 384.0 * i as f32;
            let y = 2048.0 + 384.0 * j as f32;
            vertices.push([x, y, terrain_height(x, y)]);
        }
    }
    let mut corners: Vec<[i16; 3]> = Vec::new();
    for j in 0..3i16 {
        for i in 0..3i16 {
            let a = j * 4 + i;
            corners.push([a, a + 1, a + 5]);
            corners.push([a, a + 5, a + 4]);
        }
    }
    let triangles = corners
        .iter()
        .map(|t| {
            let mut edges = [-1i16; 3];
            for (e, edge) in edges.iter_mut().enumerate() {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                if let Some(other) = corners
                    .iter()
                    .position(|o| (0..3).any(|k| o[k] == b && o[(k + 1) % 3] == a))
                {
                    *edge = other as i16;
                }
            }
            NavTriangle {
                vertices: *t,
                edges,
                flags: 0,
            }
        })
        .collect();
    let mut nav = Navmesh::new(form, cell, vertices, triangles);
    nav.cover_triangles = vec![0];
    nav.door_links = vec![NavDoorLink {
        door: FormId(door),
        triangle: 1,
    }];
    nav
}

/// The human skeleton file: the bones of `nif::fixture::skeleton_bones`
/// (so `nif::extract_skeleton` gives exactly what it gives for
/// `nif::fixture::skeleton_nif`), and under the root one small marker box,
/// as the real skeleton carries a little geometry of its own. NPC
/// references name the skeleton as their model, so it is a mesh too.
pub fn human_skeleton() -> Vec<u8> {
    let bones = skeleton_bones();
    fn node(bones: &[(&str, Option<usize>, nif::Transform)], i: usize) -> NodeDesc {
        let (name, _, transform) = bones[i];
        let kind = if i == 0 { "BSFadeNode" } else { "NiNode" };
        let mut desc = NodeDesc::new(kind, name).with_transform(transform);
        for j in (0..bones.len()).filter(|&j| bones[j].1 == Some(i)) {
            desc = desc.with_node(node(bones, j));
        }
        desc
    }
    node(&bones, 0)
        .with_shape(box_shape(
            "SkeletonMarker",
            [1.0, 0.5, 2.0],
            "textures\\minitown\\crate.dds",
        ))
        .to_nif()
}

/// `nif::fixture::skinned_body` with a texture set (diffuse `texture` and
/// its `_n` normal map) on its shape; every other block is as that file
/// has it.
///
/// # Panics
///
/// When the `nif` fixture does not parse, which its own tests rule out.
pub fn textured_skinned_body(texture: &str) -> Vec<u8> {
    let file = nif::parse(&skinned_body()).expect("the nif skinned body fixture parses");
    let mut builder = NifBuilder::new();
    for entry in &file.blocks {
        builder.push(&entry.type_name, &entry.block);
    }
    let set = builder.push(
        "BSShaderTextureSet",
        &BsShaderTextureSet {
            textures: texture_slots(texture),
        }
        .into(),
    );
    let shader = builder.push(
        "BSShaderPPLightingProperty",
        &BsShaderPpLighting {
            texture_set: Some(set),
            ..BsShaderPpLighting::default()
        }
        .into(),
    );
    for (index, entry) in file.blocks.iter().enumerate() {
        if let Block::TriShape(shape) = &entry.block {
            let mut shape = (**shape).clone();
            shape.av.properties.push(Some(shader));
            builder.set(index as u32, &entry.type_name, &Block::tri_shape(shape));
        }
    }
    for root in &file.roots {
        builder.add_root(*root);
    }
    builder.build()
}

/// A standing idle for the human skeleton, [`STAND_DURATION`] seconds,
/// looping, accumulation root `Bip01`, text keys `start` and `end`:
/// `Bip01` keyed at the origin throughout (no travel), `Bip01 NonAccum`
/// lifted to `(0, 0, 50)` as in `nif::fixture::walk_kf`, and a slight
/// sway of `Bip01 Spine` about +X (out and back by 0.1 rad).
pub fn stand_kf() -> Vec<u8> {
    let identity = QuatTransform::default();
    let root = KfTrack::Keyed {
        bone: "Bip01".to_owned(),
        transform: QuatTransform {
            translation: [-f32::MAX; 3],
            ..identity
        },
        data: Some(NiTransformData {
            translations: linear_translations(&[(0.0, [0.0; 3]), (STAND_DURATION, [0.0; 3])]),
            ..NiTransformData::default()
        }),
    };
    let hips = KfTrack::Keyed {
        bone: "Bip01 NonAccum".to_owned(),
        transform: QuatTransform {
            translation: [0.0, 0.0, 50.0],
            ..identity
        },
        data: None,
    };
    let (s, c) = 0.05f32.sin_cos();
    let spine = KfTrack::Keyed {
        bone: "Bip01 Spine".to_owned(),
        transform: unset_transform(),
        data: Some(linear_rotations(&[
            (0.0, [1.0, 0.0, 0.0, 0.0]),
            (STAND_DURATION / 2.0, [c, s, 0.0, 0.0]),
            (STAND_DURATION, [1.0, 0.0, 0.0, 0.0]),
        ])),
    };
    KfDesc {
        text_keys: vec![
            (0.0, "start".to_owned()),
            (STAND_DURATION, "end".to_owned()),
        ],
        ..KfDesc::new("Idle", STAND_DURATION)
    }
    .with_track(root)
    .with_track(hips)
    .with_track(spine)
    .to_kf()
}

/// The time of the `Hit` text key of [`punch_kf`] and [`fire_kf`].
pub const PUNCH_HIT: f32 = 0.3;
/// Length of [`punch_kf`].
pub const PUNCH_DURATION: f32 = 0.8;
/// Length of [`fall_kf`].
pub const FALL_DURATION: f32 = 1.0;
/// How far back along −Y the body of [`fall_kf`] ends from where it
/// stood: the root travel a baked fall draws.
pub const FALL_TRAVEL: f32 = 40.0;
/// The run's stride per cycle ([`run_kf`]), units along +Y.
pub const RUN_STRIDE: f32 = 200.0;
/// The run's cycle, seconds.
pub const RUN_DURATION: f32 = 0.8;
/// The sheriff's revolver (`WEAP`, animation type 3: a one-hand pistol).
/// Its model is the tin can's, so it converts nothing new.
pub const REVOLVER_MODEL: &str = "meshes\\minitown\\tincan.nif";

/// The hold of `Bip01 NonAccum` at hip height that every fixture
/// animation keys.
fn hips_track() -> KfTrack {
    KfTrack::Keyed {
        bone: "Bip01 NonAccum".to_owned(),
        transform: QuatTransform {
            translation: [0.0, 0.0, 50.0],
            ..QuatTransform::default()
        },
        data: None,
    }
}

/// The accumulation root keyed along `keys`.
fn root_track(keys: &[(f32, [f32; 3])]) -> KfTrack {
    KfTrack::Keyed {
        bone: "Bip01".to_owned(),
        transform: QuatTransform {
            translation: [-f32::MAX; 3],
            ..QuatTransform::default()
        },
        data: Some(NiTransformData {
            translations: linear_translations(keys),
            ..NiTransformData::default()
        }),
    }
}

/// A one-shot arm swing of the human skeleton: `Bip01 L UpperArm` turns a
/// quarter about +X and back over `duration`, text keys `start`, `Hit` at
/// `hit` and `end`; clamped (it does not loop).
fn swing_kf(name: &str, duration: f32, hit: f32) -> Vec<u8> {
    let q = FRAC_PI_4 / 2.0;
    let arm = KfTrack::Keyed {
        bone: "Bip01 L UpperArm".to_owned(),
        transform: unset_transform(),
        data: Some(linear_rotations(&[
            (0.0, [1.0, 0.0, 0.0, 0.0]),
            (hit, [q.cos(), q.sin(), 0.0, 0.0]),
            (duration, [1.0, 0.0, 0.0, 0.0]),
        ])),
    };
    KfDesc {
        text_keys: vec![
            (0.0, "start".to_owned()),
            (hit, "Hit".to_owned()),
            (duration, "end".to_owned()),
        ],
        cycle_type: nif::CYCLE_CLAMP,
        ..KfDesc::new(name, duration)
    }
    .with_track(root_track(&[(0.0, [0.0; 3]), (duration, [0.0; 3])]))
    .with_track(hips_track())
    .with_track(arm)
    .to_kf()
}

/// A punch: [`swing_kf`] of [`PUNCH_DURATION`] with its `Hit` at
/// [`PUNCH_HIT`].
pub fn punch_kf() -> Vec<u8> {
    swing_kf("Punch", PUNCH_DURATION, PUNCH_HIT)
}

/// A pistol shot, as [`punch_kf`] under another name.
pub fn fire_kf() -> Vec<u8> {
    swing_kf("Fire", PUNCH_DURATION, PUNCH_HIT)
}

/// A looping pistol aim: the stand's root, the hips held, the arm raised.
pub fn aim_kf() -> Vec<u8> {
    let q = FRAC_PI_4 / 2.0;
    let arm = KfTrack::Keyed {
        bone: "Bip01 L UpperArm".to_owned(),
        transform: unset_transform(),
        data: Some(linear_rotations(&[
            (0.0, [q.cos(), q.sin(), 0.0, 0.0]),
            (1.0, [q.cos(), q.sin(), 0.0, 0.0]),
        ])),
    };
    KfDesc::new("Aim", 1.0)
        .with_track(root_track(&[(0.0, [0.0; 3]), (1.0, [0.0; 3])]))
        .with_track(hips_track())
        .with_track(arm)
        .to_kf()
}

/// A run: the walk's shape, [`RUN_STRIDE`] units along +Y in
/// [`RUN_DURATION`] seconds, looping.
pub fn run_kf() -> Vec<u8> {
    KfDesc::new("Run", RUN_DURATION)
        .with_track(root_track(&[
            (0.0, [0.0; 3]),
            (RUN_DURATION, [0.0, RUN_STRIDE, 0.0]),
        ]))
        .with_track(hips_track())
        .to_kf()
}

/// A fall onto the back: over [`FALL_DURATION`] the hips drop from 50 to
/// 5 units and the spine tips a quarter turn back about +X, while the
/// root moves [`FALL_TRAVEL`] units back along −Y; clamped.
pub fn fall_kf() -> Vec<u8> {
    let half = std::f32::consts::FRAC_PI_2 / 2.0;
    let hips = KfTrack::Keyed {
        bone: "Bip01 NonAccum".to_owned(),
        transform: unset_transform(),
        data: Some(NiTransformData {
            translations: linear_translations(&[
                (0.0, [0.0, 0.0, 50.0]),
                (FALL_DURATION, [0.0, 0.0, 5.0]),
            ]),
            ..linear_rotations(&[
                (0.0, [1.0, 0.0, 0.0, 0.0]),
                (FALL_DURATION, [half.cos(), -half.sin(), 0.0, 0.0]),
            ])
        }),
    };
    KfDesc {
        cycle_type: nif::CYCLE_CLAMP,
        ..KfDesc::new("Fall", FALL_DURATION)
    }
    .with_track(root_track(&[
        (0.0, [0.0; 3]),
        (FALL_DURATION, [0.0, -FALL_TRAVEL, 0.0]),
    ]))
    .with_track(hips)
    .to_kf()
}

/// Writes the synthetic install under `dir` (`dir/Data/...`) and returns
/// what it holds.
///
/// One worldspace of 3 × 3 exterior cells with a sloped terrain and one
/// steep ridge, two land textures on two layers, a navmesh in the centre
/// cell, a multi-part building with a teleport door into one interior (and
/// back), a container, a few props (one rotated, one scaled, one with an
/// enable parent), a race, a hair, outfits with an addon and leveled
/// lists, two NPCs (one templated) and a creature, the human skeleton, a
/// skinned body part and a stand and a walk animation (see
/// [`MiniTown::animated_actor`]), a script with source text, a
/// quest with stages and an objective, and a dialogue topic. One prop's
/// model is missing from the archives and one texture is corrupt, so that
/// both failure paths are exercised.
///
/// # Panics
///
/// When the files cannot be written.
pub fn mini_town(dir: &Path) -> MiniTown {
    let data = dir.join("Data");
    fs::create_dir_all(&data).expect("create the data directory");

    // Forms. The plugin has no masters, so every id is its own.
    const WORLD: u32 = 0x800;
    const PERSISTENT_CELL: u32 = 0x801;
    const CELL0: u32 = 0x810; // + (x + 1) * 3 + (y + 1)
    const LAND0: u32 = 0x820;
    const NAVMESH: u32 = 0x830;
    const LTEX_DIRT: u32 = 0x840;
    const LTEX_GRASS: u32 = 0x841;
    const TXST_DIRT: u32 = 0x842;
    const TXST_GRASS: u32 = 0x843;
    const HOUSE: u32 = 0x850;
    const FENCE: u32 = 0x851;
    const ROCK: u32 = 0x852;
    const SIGN: u32 = 0x853;
    const DOOR: u32 = 0x858;
    const CRATE: u32 = 0x859;
    const TIN_CAN: u32 = 0x85A;
    const BOTTLE: u32 = 0x85B;
    const LOST: u32 = 0x85C;
    const DUSTER: u32 = 0x85D;
    const RACE: u32 = 0x860;
    const HAIR: u32 = 0x861;
    const EYES: u32 = 0x862;
    const SHERIFF: u32 = 0x863;
    const SCRIPT: u32 = 0x864;
    const QUEST: u32 = 0x865;
    const TOPIC: u32 = 0x866;
    const INFO: u32 = 0x867;
    const INTERIOR_CELL: u32 = 0x870;
    const HAT: u32 = 0x880;
    const VEST: u32 = 0x881;
    const GLOVE: u32 = 0x882;
    const GLOVE_LIST: u32 = 0x883;
    const OUTFIT_LIST: u32 = 0x884;
    const FARMER: u32 = 0x885;
    const SETTLERS: u32 = 0x886;
    const DEPUTY: u32 = 0x887;
    const DOG: u32 = 0x888;
    const GOGGLES: u32 = 0x889;
    const TRIGGER: u32 = 0x88A;
    const REVOLVER: u32 = 0x88B;
    const R_HOUSE: u32 = 0x900;
    const R_FENCE: u32 = 0x901;
    const R_ROCK: u32 = 0x902;
    const R_CRATE: u32 = 0x903;
    const R_TIN_CAN: u32 = 0x904;
    const R_BOTTLE: u32 = 0x905;
    const R_LOST: u32 = 0x906;
    const R_SIGN: u32 = 0x907;
    const R_FENCE2: u32 = 0x908;
    const R_SHERIFF: u32 = 0x910;
    const R_DOOR_OUT: u32 = 0x920;
    const R_DOOR_IN: u32 = 0x921;
    const R_INSIDE_CRATE: u32 = 0x922;
    const R_INSIDE_WALL: u32 = 0x923;
    const R_INSIDE_CAN: u32 = 0x924;
    const R_DEPUTY: u32 = 0x925;
    const R_DOG: u32 = 0x926;
    const R_TRIGGER: u32 = 0x927;

    let cell_id = |x: i32, y: i32| CELL0 + ((x + 1) * 3 + (y + 1)) as u32;
    let land_id = |x: i32, y: i32| LAND0 + ((x + 1) * 3 + (y + 1)) as u32;
    let ground = |x: f32, y: f32| [x, y, terrain_height(x, y)];

    let mut plugin = PluginBuilder::new();
    plugin.master_file(true).author("mojave-foundry fixture");

    // Textures and base objects.
    plugin.add(TextureSet::new(
        TXST_DIRT,
        "MTDirtSet",
        "landscape\\mtdirt.dds",
    ));
    plugin.add(TextureSet::new(
        TXST_GRASS,
        "MTGrassSet",
        "Landscape/MTGrass.dds",
    ));
    plugin.add(LandTexture::new(LTEX_DIRT, "MTDirt", TXST_DIRT));
    plugin.add(LandTexture::new(LTEX_GRASS, "MTGrass", TXST_GRASS));
    plugin.add(Static::new(HOUSE, "MTHouse", "minitown\\house.nif"));
    plugin.add(Static::new(FENCE, "MTFence", "MiniTown\\Fence.NIF"));
    plugin.add(Static::new(ROCK, "MTRock", "minitown\\rock.nif"));
    plugin.add(Static::new(SIGN, "MTSign", "minitown\\sign.nif"));
    // A trigger activator; its model stands in for the volume in an editor.
    plugin.add_raw(
        RawRecord::new(b"ACTI", TRIGGER)
            .with(b"EDID", &b"MTTownTrigger\0"[..])
            .with(b"MODL", &b"minitown\\fence.nif\0"[..]),
    );
    let mut door = Door::new(DOOR, "MTHouseDoor", "Door", "minitown\\door.nif");
    door.common.script = Some(FormId(SCRIPT));
    plugin.add(door);
    plugin.add(Container::new(
        CRATE,
        "MTCrate",
        "Crate",
        "minitown\\crate.nif",
        vec![
            ContainerItem::new(TIN_CAN, 3),
            ContainerItem::new(BOTTLE, 1),
        ],
    ));
    plugin.add(MiscItem::new(
        TIN_CAN,
        "MTTinCan",
        "Tin Can",
        "minitown\\tincan.nif",
        1,
        0.5,
    ));
    plugin.add(MiscItem::new(
        BOTTLE,
        "MTBottle",
        "Bottle",
        "minitown\\bottle.nif",
        2,
        1.0,
    ));
    plugin.add(MiscItem::new(
        LOST,
        "MTLostCrate",
        "Lost Crate",
        "minitown\\lostcrate.nif",
        5,
        3.0,
    ));
    let armour = |form: u32, editor_id: &str, slots: u32, model: &str| Armor {
        common: fnv_esm::Common::new(form)
            .with_editor_id(editor_id)
            .with_name(editor_id.trim_start_matches("MT"))
            .with_model(model),
        biped: Some(BipedData {
            biped_flags: slots,
            general_flags: 0,
        }),
        data: Some(ArmorData {
            value: 40,
            health: 100,
            weight: 4.0,
        }),
        ..Armor::default()
    };
    // The duster has no female model: a woman wears the male one.
    plugin.add(Armor {
        biped_model_list: Some(FormId(GLOVE_LIST)),
        ..armour(
            DUSTER,
            "MTDuster",
            BipedData::SLOT_UPPER_BODY,
            "MiniTown\\Armor\\Duster.NIF",
        )
    });
    plugin.add(armour(
        HAT,
        "MTHat",
        BipedData::SLOT_HAIR | 0x400,
        "minitown\\armor\\hat.nif",
    ));
    plugin.add(armour(
        VEST,
        "MTVest",
        BipedData::SLOT_UPPER_BODY,
        "minitown\\armor\\vest.nif",
    ));
    plugin.add(armour(
        GOGGLES,
        "MTGoggles",
        0x800,
        "minitown\\armor\\goggles.nif",
    ));
    plugin.add(ArmorAddon {
        female_model: Some("minitown\\armor\\glove_f.nif".to_owned()),
        ..ArmorAddon::new(
            GLOVE,
            "MTGloveLeft",
            BipedData::SLOT_LEFT_HAND,
            "minitown\\armor\\glove.nif",
        )
    });
    plugin.add(FormList::new(
        GLOVE_LIST,
        "MTGloveList",
        vec![FormId(GLOVE)],
    ));
    plugin.add(LeveledList::new(
        LeveledKind::Item,
        OUTFIT_LIST,
        "MTOutfitList",
        vec![
            LeveledEntry::new(3, DUSTER, 1),
            LeveledEntry::new(1, GOGGLES, 1),
            LeveledEntry::new(1, HAT, 1),
        ],
    ));
    let part = |index: u32, model: Option<&str>, texture: Option<&str>| {
        RacePart::new(index, model, texture)
    };
    let mut race = Race::new(RACE, "MTRace", "Townsfolk");
    race.data = Some(RaceData {
        height: [1.0, 1.0],
        weight: [1.0, 1.0],
        flags: 1,
        ..RaceData::default()
    });
    race.default_hair = Some([FormId(HAIR), FormId(HAIR)]);
    race.head = Some(RaceParts {
        male: vec![
            part(
                0,
                Some("MiniTown\\Characters\\HeadMale.NIF"),
                Some("MiniTown\\Characters\\HeadMale.dds"),
            ),
            part(1, None, Some("MiniTown\\Characters\\Ears.dds")),
        ],
        female: vec![part(
            0,
            Some("MiniTown\\Characters\\HeadFemale.NIF"),
            Some("MiniTown\\Characters\\HeadFemale.dds"),
        )],
    });
    let body = |sex: &str| {
        vec![
            part(
                0,
                Some(&format!("MiniTown\\Characters\\UpperBody{sex}.nif")),
                Some(&format!("MiniTown\\Characters\\Body{sex}.dds")),
            ),
            part(
                1,
                Some(&format!("MiniTown\\Characters\\LeftHand{sex}.nif")),
                Some(&format!("MiniTown\\Characters\\Hand{sex}.dds")),
            ),
            part(
                2,
                Some(&format!("MiniTown\\Characters\\RightHand{sex}.nif")),
                Some(&format!("MiniTown\\Characters\\Hand{sex}.dds")),
            ),
            part(
                3,
                Some(&format!("MiniTown\\Characters\\UpperBody{sex}.egt")),
                None,
            ),
        ]
    };
    race.body = Some(RaceParts {
        male: body("Male"),
        female: body("Female"),
    });
    race.hairs = vec![FormId(HAIR)];
    race.eyes = vec![FormId(EYES)];
    plugin.add(race);
    plugin.add(Hair::new(
        HAIR,
        "MTHair",
        "MiniTown\\Characters\\Hair.NIF",
        "MiniTown\\Characters\\Hair.dds",
    ));
    plugin.add(Eyes::new(EYES, "MTEyes", "MiniTown\\Characters\\Eyes.dds"));
    let mut sheriff = Npc::new(SHERIFF, "MTSheriff", "Sheriff");
    sheriff.common.model = Some("Characters\\_Male\\Skeleton.NIF".to_owned());
    sheriff.stats = Some(NpcStats {
        base_health: 80,
        attributes: [5; 7],
    });
    if let Some(base) = &mut sheriff.base {
        base.level = 6;
    }
    sheriff.race = Some(FormId(RACE));
    sheriff.hair = Some(FormId(HAIR));
    sheriff.eyes = Some(FormId(EYES));
    sheriff.hair_color = Some(Rgb {
        r: 90,
        g: 60,
        b: 30,
    });
    sheriff.facegen_geometry = Some((0..200u8).collect());
    sheriff.facegen_asymmetry = Some(vec![7; 120]);
    sheriff.facegen_texture = Some(vec![3; 200]);
    sheriff.inventory = vec![
        ContainerItem::new(DUSTER, 1),
        ContainerItem::new(HAT, 1),
        ContainerItem::new(TIN_CAN, 2),
        ContainerItem::new(VEST, 1),
        ContainerItem::new(REVOLVER, 1),
    ];
    plugin.add(sheriff);
    let mut dnam = vec![0u8; 136];
    dnam[0..4].copy_from_slice(&3u32.to_le_bytes());
    plugin.add_raw(
        RawRecord::new(b"WEAP", REVOLVER)
            .with(b"EDID", b"MTRevolver\0".to_vec())
            .with(b"MODL", b"MiniTown\\TinCan.nif\0".to_vec())
            .with(b"DNAM", dnam),
    );
    let mut farmer = Npc::new(FARMER, "MTFarmer", "Farmer");
    farmer.common.model = Some("Characters\\_Male\\Skeleton.NIF".to_owned());
    farmer.base = Some(ActorBase {
        flags: 0x1,
        level: 2,
        ..ActorBase::default()
    });
    farmer.race = Some(FormId(RACE));
    farmer.hair = Some(FormId(HAIR));
    farmer.hair_color = Some(Rgb {
        r: 200,
        g: 180,
        b: 60,
    });
    farmer.stats = Some(NpcStats {
        base_health: 30,
        attributes: [4; 7],
    });
    farmer.inventory = vec![ContainerItem::new(DUSTER, 1)];
    plugin.add(farmer);
    plugin.add(LeveledList::new(
        LeveledKind::Npc,
        SETTLERS,
        "MTSettlers",
        vec![
            LeveledEntry::new(5, SHERIFF, 1),
            LeveledEntry::new(1, FARMER, 1),
            LeveledEntry::new(1, SHERIFF, 1),
        ],
    ));
    // Traits, model and base data from the template; stats and inventory
    // its own.
    let mut deputy = Npc::new(DEPUTY, "MTDeputy", "Deputy");
    deputy.base = Some(ActorBase {
        level: 3,
        template_flags: (crate::actors::USE_TRAITS
            | crate::actors::USE_MODEL
            | crate::actors::USE_BASE_DATA),
        ..ActorBase::default()
    });
    deputy.template = Some(FormId(SETTLERS));
    deputy.stats = Some(NpcStats {
        base_health: 60,
        attributes: [6; 7],
    });
    deputy.inventory = vec![
        ContainerItem::new(OUTFIT_LIST, 1),
        ContainerItem::new(0x0000_0FFFu32, 1),
    ];
    plugin.add(deputy);
    let mut dog = Creature::new(
        DOG,
        "MTDog",
        "Dog",
        "MiniTown\\Creatures\\Dog\\Skeleton.NIF",
    );
    dog.model_list = vec!["DogBody.NIF".to_owned(), "DogTail.nif".to_owned()];
    dog.data = Some(CreatureData {
        health: 25,
        ..CreatureData::default()
    });
    plugin.add(dog);
    plugin.add(Script::new(
        SCRIPT,
        "MTDoorScript",
        "scn MTDoorScript\n\nshort knocked\n\nbegin OnActivate\n  if knocked == 0 ; first visit\n    set knocked to 1\n    SetStage MTMainQuest 10\n    PlaySound \"door knock\"\n  endif\n  Activate\nend\n",
    ));
    let mut quest = Quest::new(QUEST, "MTMainQuest", "Settling In");
    quest.common.script = Some(FormId(SCRIPT));
    quest.conditions = vec![Condition::float(58, Comparison::GreaterOrEqual, 1.0)];
    let mut arrive = QuestLogEntry::new("You arrived in the mini town.");
    arrive.script = ScriptBody::from_source("SetObjectiveDisplayed MTMainQuest 10 1");
    let mut settled = QuestLogEntry::new("You settled in.");
    settled.conditions = vec![Condition::float(72, Comparison::Equal, 1.0).with_params(SHERIFF, 0)];
    quest.stages = vec![
        QuestStage {
            index: 10,
            entries: vec![arrive],
        },
        QuestStage {
            index: 20,
            entries: vec![settled],
        },
    ];
    quest.objectives = vec![QuestObjective {
        index: 10,
        description: Some("Visit the house.".to_owned()),
        targets: Vec::new(),
    }];
    plugin.add(quest);
    let mut info = Info::new(INFO, QUEST, vec![Response::new(1, "Welcome to town.")]);
    info.conditions = vec![
        Condition::float(72, Comparison::Equal, 1.0).with_params(SHERIFF, 0),
        Condition::float(58, Comparison::Less, 20.0),
    ];
    info.result_begin = ScriptBody::from_source("SetStage MTMainQuest 20");
    plugin.topic(Topic::new(TOPIC, "MTGreeting", "Hello", QUEST), vec![info]);

    // The worldspace.
    let mut world = WorldSpec::new(World::new(WORLD, WORLDSPACE));
    let door_out_pos = ground(2048.0, 2560.0 - 230.0);
    let door_in_pos = [0.0, -200.0, 0.0];
    let door_out = PlacedRef::new(PlacedKind::Object, R_DOOR_OUT, DOOR, door_out_pos, [0.0; 3])
        .with_teleport(R_DOOR_IN, [0.0, -100.0, 0.0], [0.0, 0.0, 0.0])
        .persistent();
    let sheriff_ref = PlacedRef::new(
        PlacedKind::Npc,
        R_SHERIFF,
        SHERIFF,
        ground(5000.0, 2000.0),
        [0.0, 0.0, 1.0],
    )
    .persistent();
    let mut persistent = CellSpec::new(Cell::exterior(PERSISTENT_CELL, 0, 0));
    persistent.persistent = vec![door_out, sheriff_ref];
    world = world.with_persistent_cell(persistent);

    for x in -1..=1 {
        for y in -1..=1 {
            let mut cell = Cell::exterior(cell_id(x, y), x, y);
            cell.common.editor_id = Some(format!("MTCell{}{}", x + 1, y + 1));
            let mut land = Land::new(land_id(x, y)).with_heights(&land_heights(x, y));
            land.flags = Some(1);
            land.normals = Some(fnv_esm::fixture::up_normals());
            land.colors = Some(solid_colors([200, 190, 170]));
            land.base_layers = (0..4)
                .map(|quadrant| BaseLayer {
                    texture: FormId(LTEX_DIRT),
                    quadrant,
                    layer: -1,
                })
                .collect();
            land.alpha_layers = [0u8, 3]
                .iter()
                .map(|&quadrant| AlphaLayer {
                    texture: FormId(LTEX_GRASS),
                    quadrant,
                    layer: 0,
                    opacity: (0..17u16)
                        .map(|i| AlphaPoint {
                            position: i * 17 + i,
                            opacity: f32::from(i) / 16.0,
                        })
                        .collect(),
                })
                .collect();
            if (x, y) == (0, 0) {
                land.common.meta.flags.0 |= RecordFlags::COMPRESSED;
            }
            let mut spec = CellSpec::new(cell).with_land(land);
            let ox = x as f32 * 4096.0;
            let oy = y as f32 * 4096.0;
            match (x, y) {
                (0, 0) => {
                    spec = spec
                        .with_navmesh(navmesh(NAVMESH, cell_id(0, 0), R_DOOR_OUT))
                        .with_temporary(PlacedRef::new(
                            PlacedKind::Object,
                            R_HOUSE,
                            HOUSE,
                            ground(2048.0, 2560.0),
                            [0.0; 3],
                        ))
                        .with_temporary(PlacedRef::new(
                            PlacedKind::Object,
                            R_CRATE,
                            CRATE,
                            ground(1200.0, 1200.0),
                            [0.0, 0.0, 0.5],
                        ));
                }
                (1, 0) => {
                    spec = spec
                        .with_temporary(PlacedRef::new(
                            PlacedKind::Object,
                            R_FENCE,
                            FENCE,
                            ground(ox + 1000.0, oy + 1000.0),
                            [0.0, 0.0, FRAC_PI_4],
                        ))
                        .with_visible_distant(PlacedRef::new(
                            PlacedKind::Object,
                            R_FENCE2,
                            FENCE,
                            ground(ox + 2000.0, oy + 1000.0),
                            [0.0; 3],
                        ));
                }
                (-1, 0) => {
                    spec = spec.with_temporary(
                        PlacedRef::new(
                            PlacedKind::Object,
                            R_ROCK,
                            ROCK,
                            ground(ox + 3000.0, oy + 3000.0),
                            [0.0; 3],
                        )
                        .with_scale(2.0),
                    );
                }
                (0, -1) => {
                    spec = spec.with_temporary(
                        PlacedRef::new(
                            PlacedKind::Object,
                            R_TIN_CAN,
                            TIN_CAN,
                            ground(ox + 500.0, oy + 600.0),
                            [0.0; 3],
                        )
                        .initially_disabled(),
                    );
                }
                (1, 1) => {
                    spec = spec.with_temporary(
                        PlacedRef::new(
                            PlacedKind::Object,
                            R_BOTTLE,
                            BOTTLE,
                            ground(ox + 700.0, oy + 800.0),
                            [0.0; 3],
                        )
                        .with_enable_parent(R_ROCK, true),
                    );
                }
                (-1, -1) => {
                    spec = spec.with_temporary(PlacedRef::new(
                        PlacedKind::Object,
                        R_LOST,
                        LOST,
                        ground(ox + 2000.0, oy + 2000.0),
                        [0.0; 3],
                    ));
                }
                (-1, 1) => {
                    spec = spec.with_temporary(
                        PlacedRef::new(
                            PlacedKind::Object,
                            R_TRIGGER,
                            TRIGGER,
                            ground(ox + 2000.0, oy + 2000.0),
                            [0.0, 0.0, 0.25],
                        )
                        .with_primitive(Primitive {
                            bounds: [256.0, 192.0, 128.0],
                            color: [0.8, 0.4, 0.1],
                            unknown: 0.15,
                            shape: Primitive::SHAPE_BOX,
                        }),
                    );
                }
                (0, 1) => {
                    spec = spec.with_temporary(PlacedRef::new(
                        PlacedKind::Object,
                        R_SIGN,
                        SIGN,
                        ground(ox + 1800.0, oy + 300.0),
                        [0.0, 0.0, 3.0],
                    ));
                }
                _ => {}
            }
            world = world.with_cell(spec);
        }
    }
    plugin.world(world);

    // The interior.
    let mut interior = Cell::interior(INTERIOR_CELL, INTERIOR);
    interior.common.full_name = Some("House".to_owned());
    let inside = CellSpec::new(interior)
        .with_persistent(
            PlacedRef::new(PlacedKind::Object, R_DOOR_IN, DOOR, door_in_pos, [0.0; 3])
                .with_teleport(R_DOOR_OUT, door_out_pos, [0.0, 0.0, 3.1])
                .persistent(),
        )
        .with_temporary(PlacedRef::new(
            PlacedKind::Object,
            R_INSIDE_CRATE,
            CRATE,
            [100.0, 100.0, 0.0],
            [0.0; 3],
        ))
        .with_temporary(PlacedRef::new(
            PlacedKind::Object,
            R_INSIDE_WALL,
            FENCE,
            [0.0, 250.0, 0.0],
            [0.0; 3],
        ))
        .with_temporary(PlacedRef::new(
            PlacedKind::Object,
            R_INSIDE_CAN,
            TIN_CAN,
            [120.0, 100.0, 60.0],
            [0.0; 3],
        ))
        .with_temporary(PlacedRef::new(
            PlacedKind::Npc,
            R_DEPUTY,
            DEPUTY,
            [-80.0, 60.0, 0.0],
            [0.0, 0.0, 1.5],
        ))
        .with_temporary(PlacedRef::new(
            PlacedKind::Creature,
            R_DOG,
            DOG,
            [-40.0, 120.0, 0.0],
            [0.0; 3],
        ));
    plugin.interior(inside);

    fs::write(data.join(PLUGIN), plugin.build()).expect("write the plugin");

    // Meshes: a loose sign, the rest archived.
    let mut meshes = BsaBuilder::new();
    meshes.compressed(true);
    meshes.add(
        "meshes\\minitown\\house.nif",
        &building_scene(
            [600.0, 400.0, 300.0],
            "textures\\minitown\\wall.dds",
            "textures\\minitown\\roof.dds",
        )
        .with_unknown("BSFurnitureMarker", vec![0; 12])
        .to_nif(),
    );
    meshes.add(
        "meshes\\minitown\\fence.nif",
        &box_mesh([500.0, 20.0, 100.0], "textures\\minitown\\fence.dds"),
    );
    meshes.add(
        "meshes\\minitown\\rock.nif",
        &box_mesh([80.0, 70.0, 50.0], "textures\\minitown\\rock.dds"),
    );
    meshes.add(
        "meshes\\minitown\\door.nif",
        &box_mesh([100.0, 10.0, 200.0], "textures\\minitown\\door.dds"),
    );
    meshes.add(
        "meshes\\minitown\\crate.nif",
        &box_mesh([60.0, 60.0, 60.0], "textures\\minitown\\crate.dds"),
    );
    meshes.add(
        "meshes\\minitown\\tincan.nif",
        &box_mesh([8.0, 8.0, 12.0], "textures\\minitown\\crate.dds"),
    );
    // Same bytes as the tin can: one converted file serves both paths.
    meshes.add(
        "meshes\\minitown\\bottle.nif",
        &box_mesh([8.0, 8.0, 12.0], "textures\\minitown\\crate.dds"),
    );
    // Actors: the human skeleton and one skinned part from the `nif`
    // character fixtures; the race's other parts, hair, outfits and a
    // creature as boxes. Each mesh has its own size so that no two share
    // their bytes.
    let skin = "textures\\minitown\\characters\\skin.dds";
    let cloth = "textures\\minitown\\armor\\cloth.dds";
    meshes.add(crate::actors::HUMAN_SKELETON, &human_skeleton());
    meshes.add(SKINNED_PART, &textured_skinned_body(skin));
    let actor_meshes: [(&str, f32, &str); 14] = [
        ("minitown\\characters\\headmale.nif", 20.0, skin),
        ("minitown\\characters\\headfemale.nif", 19.0, skin),
        ("minitown\\characters\\upperbodymale.nif", 60.0, skin),
        ("minitown\\characters\\upperbodyfemale.nif", 55.0, skin),
        ("minitown\\characters\\lefthandmale.nif", 9.0, skin),
        ("minitown\\characters\\lefthandfemale.nif", 8.0, skin),
        ("minitown\\characters\\righthandfemale.nif", 8.5, skin),
        (
            "minitown\\characters\\hair.nif",
            22.0,
            "textures\\minitown\\characters\\hair.dds",
        ),
        ("minitown\\armor\\duster.nif", 65.0, cloth),
        ("minitown\\armor\\hat.nif", 30.0, cloth),
        ("minitown\\armor\\glove.nif", 10.0, cloth),
        ("minitown\\armor\\goggles.nif", 12.0, cloth),
        (
            "minitown\\creatures\\dog\\skeleton.nif",
            2.0,
            "textures\\minitown\\crate.dds",
        ),
        (
            "minitown\\creatures\\dog\\dogbody.nif",
            40.0,
            "textures\\minitown\\creatures\\dog.dds",
        ),
    ];
    for (path, size, texture) in actor_meshes {
        meshes.add(
            &format!("meshes\\{path}"),
            &box_mesh([size, size * 0.5, size * 2.0], texture),
        );
    }
    // The stand and the walk. There is no female walk, so women fall back
    // to the male one.
    meshes.add(crate::actors::HUMAN_STAND, &stand_kf());
    meshes.add(crate::actors::HUMAN_WALK_MALE, &walk_kf());
    // A few combat animations under the source's names: a punch and a
    // pistol shot with `Hit` keys, a run, a pistol aim and a fall back.
    let folder = crate::actors::HUMAN_ANIMATIONS;
    for (name, bytes) in [
        ("h2hattackright_a.kf", punch_kf()),
        ("locomotion\\male\\mtfastforward.kf", run_kf()),
        ("idleanims\\mt_knockdownfaceup.kf", fall_kf()),
        ("1hpaim.kf", aim_kf()),
        ("1hpattackright.kf", fire_kf()),
    ] {
        meshes.add(&format!("{folder}{name}"), &bytes);
    }
    fs::write(data.join("MiniTown - Meshes.bsa"), meshes.build()).expect("write the mesh archive");
    let sign = NodeDesc::root("Sign")
        .with_shape(quad_shape(
            "Sign:0",
            [120.0, 80.0],
            "textures\\minitown\\sign.dds",
        ))
        .with_unknown("BSFurnitureMarker", vec![0; 12])
        .to_nif();
    let loose = data.join("Meshes").join("MiniTown");
    fs::create_dir_all(&loose).expect("create the loose mesh folder");
    fs::write(loose.join("Sign.nif"), sign).expect("write the loose mesh");

    let mut textures = BsaBuilder::new();
    let colours: [(&str, [u8; 3]); 8] = [
        ("landscape\\mtdirt", [150, 110, 70]),
        ("landscape\\mtgrass", [90, 140, 60]),
        ("minitown\\wall", [200, 200, 190]),
        ("minitown\\roof", [140, 50, 40]),
        ("minitown\\fence", [120, 90, 60]),
        ("minitown\\rock", [128, 128, 128]),
        ("minitown\\door", [100, 70, 40]),
        ("minitown\\crate", [170, 130, 80]),
    ];
    for (name, rgb) in colours {
        textures.add(&format!("textures\\{name}.dds"), &bc1(rgb));
        if name.starts_with("minitown") {
            textures.add(&format!("textures\\{name}_n.dds"), &flat_normal_map());
        }
    }
    let actor_colours: [(&str, [u8; 3]); 4] = [
        ("minitown\\characters\\skin", [210, 160, 130]),
        ("minitown\\characters\\hair", [80, 60, 40]),
        ("minitown\\armor\\cloth", [110, 90, 70]),
        ("minitown\\creatures\\dog", [150, 120, 90]),
    ];
    for (name, rgb) in actor_colours {
        textures.add(&format!("textures\\{name}.dds"), &bc1(rgb));
        textures.add(&format!("textures\\{name}_n.dds"), &flat_normal_map());
    }
    // The race's skin textures, used instead of the meshes' own.
    let skins: [(&str, [u8; 3]); 6] = [
        ("headmale", [200, 150, 120]),
        ("headfemale", [205, 155, 125]),
        ("bodymale", [190, 140, 110]),
        ("bodyfemale", [195, 145, 115]),
        ("handmale", [185, 135, 105]),
        ("handfemale", [180, 130, 100]),
    ];
    for (name, rgb) in skins {
        textures.add(
            &format!("textures\\minitown\\characters\\{name}.dds"),
            &bc1(rgb),
        );
    }
    textures.add(CORRUPT_TEXTURE, b"this is not a DDS file");
    textures.add("textures\\minitown\\sign_n.dds", &flat_normal_map());
    fs::write(data.join("MiniTown - Textures.bsa"), textures.build())
        .expect("write the texture archive");
    // Never opened: sound archives cannot hold meshes or textures.
    fs::write(data.join("MiniTown - Sounds.bsa"), b"not an archive")
        .expect("write the sound archive");

    MiniTown {
        data_dir: data,
        plugins: vec![PLUGIN.to_owned()],
        slice: Slice {
            name: "mini town".to_owned(),
            worldspace: WORLDSPACE.to_owned(),
            x: -1..=1,
            y: -1..=1,
            interior_depth: 1,
        },
        exterior_door: key(R_DOOR_OUT),
        interior_door: key(R_DOOR_IN),
        interior_cell: key(INTERIOR_CELL),
        house: key(R_HOUSE),
        rotated_fence: key(R_FENCE),
        scaled_rock: key(R_ROCK),
        crate_ref: key(R_CRATE),
        bottle: key(R_BOTTLE),
        lost_crate: key(R_LOST),
        sheriff_ref: key(R_SHERIFF),
        sheriff: key(SHERIFF),
        deputy: key(DEPUTY),
        deputy_ref: key(R_DEPUTY),
        farmer: key(FARMER),
        settlers: key(SETTLERS),
        outfit_list: key(OUTFIT_LIST),
        race: key(RACE),
        dog: key(DOG),
        dog_ref: key(R_DOG),
        script: key(SCRIPT),
        quest: key(QUEST),
        topic: key(TOPIC),
        navmesh: key(NAVMESH),
        trigger_volume: key(R_TRIGGER),
        trigger_base: key(TRIGGER),
        centre_cell: key(cell_id(0, 0)),
        animated_actor: key(SHERIFF),
    }
}
