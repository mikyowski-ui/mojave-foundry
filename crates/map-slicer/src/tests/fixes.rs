//! Door stubs, furniture shaped like walls, outlines by kind, furniture
//! footprints and bodies, roofs over walkable ground, landmarks beyond the
//! engine's reach, wire props and SpeedTree skips (2026-11-23 fixes), on
//! synthetic rooms.

use std::collections::{BTreeMap, BTreeSet};

use foundry_render::Texture;
use mir::{FormKey, TeleportDoc};
use nif::AlphaMode;

use super::*;
use crate::hex::Plane;
use crate::{
    Class, DeclutterProfile, Footprint, FurnitureRule, InteriorSlice, MeshFacts, RoofCut,
    SkipReason, WireRule, classify,
};

const FLOOR_Z: f32 = 2048.0;
const HALF: f32 = 560.0;

fn on_floor() -> GroundSampler {
    sampler(|_, _| FLOOR_Z)
}

fn key(id: u32) -> String {
    FormKey::new("test.esm", id).to_string()
}

fn room_assets() -> TestAssets {
    TestAssets::new()
        .with(
            "floor",
            mesh(&nif::fixture::quad([2.0 * HALF, 2.0 * HALF], ROOF)),
        )
        .with(
            "wall",
            mesh(&nif::fixture::box_mesh([2.0 * HALF, 16.0, 200.0], WALL)),
        )
        .with(
            "door",
            mesh(&nif::fixture::box_mesh([110.0, 10.0, 180.0], WALL)),
        )
        .with(
            "shelf",
            mesh(&nif::fixture::box_mesh([700.0, 70.0, 140.0], WALL)),
        )
        .with(
            "partition",
            mesh(&nif::fixture::box_mesh([500.0, 16.0, 220.0], WALL)),
        )
        .with(
            "table",
            mesh(&nif::fixture::box_mesh([240.0, 140.0, 60.0], ROOF)),
        )
        .with(
            "crate",
            mesh(&nif::fixture::box_mesh([60.0, 60.0, 50.0], WALL)),
        )
}

fn put(id: u32, kind: &str, model: &str, hash: &str, xy: [f32; 2], yaw: f32) -> Reference {
    reference(
        id,
        kind,
        &format!("meshes\\test\\{model}.nif"),
        hash,
        &on_floor(),
        xy,
        0.0,
        yaw,
    )
}

const SOUTH_WALL: u32 = 2;
const NORTH_WALL: u32 = 3;
const SOUTH_DOOR: u32 = 30;
const NORTH_DOOR: u32 = 31;

/// A floor, four walls and a door in the south (near) and the north (far)
/// wall.
fn room() -> Vec<Reference> {
    let quarter = std::f32::consts::FRAC_PI_2;
    vec![
        put(1, "STAT", "floor", "floor", [0.0, 0.0], 0.0),
        put(SOUTH_WALL, "STAT", "wall", "wall", [0.0, -HALF], 0.0),
        put(NORTH_WALL, "STAT", "wall", "wall", [0.0, HALF], 0.0),
        put(4, "STAT", "wall", "wall", [HALF, 0.0], quarter),
        put(5, "STAT", "wall", "wall", [-HALF, 0.0], quarter),
        put(SOUTH_DOOR, "DOOR", "door", "door", [0.0, -HALF + 14.0], 0.0),
        put(NORTH_DOOR, "DOOR", "door", "door", [0.0, HALF - 14.0], 0.0),
    ]
}

fn plane() -> Plane {
    Plane::new(&projection(), window().x_west, window().y_north).unwrap()
}

/// The hexes inside the room, `margin` units clear of the walls.
fn inside(margin: f32) -> BTreeSet<i32> {
    let plane = plane();
    (0..40_000)
        .filter(|&h| {
            plane.hex_tile(h).is_some_and(|t| {
                let [x, y] = plane.source_of_tile(t);
                x.abs() <= f64::from(HALF - margin) && y.abs() <= f64::from(HALF - margin)
            })
        })
        .collect()
}

fn indoors(cut: f32, declutter: Option<DeclutterProfile>) -> SlicerConfig {
    SlicerConfig {
        interior: Some(InteriorSlice {
            cut_height: cut,
            ..InteriorSlice::at(FLOOR_Z)
        }),
        open_hexes: inside(60.0),
        declutter,
        ..SlicerConfig::default()
    }
}

fn of(sliced: &Sliced, id: u32) -> Vec<&Placement> {
    let k = key(id);
    sliced
        .placements
        .iter()
        .filter(|p| p.reference == k)
        .collect()
}

/// Opaque pixels and the topmost opaque screen row of the placements.
fn drawn(sliced: &Sliced, placements: &[&Placement]) -> (usize, i64) {
    let (mut n, mut top) = (0, i64::MAX);
    for p in placements {
        let a = &sliced.art[p.art];
        let o = frame_origin(a, p);
        for y in 0..a.height as usize {
            for x in 0..a.width as usize {
                if a.pixels[y * a.width as usize + x] != 0 {
                    n += 1;
                    top = top.min(o[1] + y as i64);
                }
            }
        }
    }
    (n, top)
}

fn readable_without_doors() -> DeclutterProfile {
    let mut p = DeclutterProfile::readable();
    if let Some(c) = &mut p.cutaway {
        c.doors = None;
    }
    p
}

#[test]
fn a_door_in_a_cut_wall_keeps_a_stub_and_one_in_a_whole_wall_stays() {
    let assets = room_assets();
    let refs = room();
    let ground = on_floor();
    let whole = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &indoors(210.0, Some(readable_without_doors())),
    )
    .unwrap();
    let stubbed = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &indoors(210.0, Some(DeclutterProfile::readable())),
    )
    .unwrap();
    // The south door stood full height in the wall cut to the kerb; now it
    // is cut lower, though not to the kerb, and still blocks its hex.
    let (before, after) = (of(&whole, SOUTH_DOOR), of(&stubbed, SOUTH_DOOR));
    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 1);
    assert_eq!(
        (after[0].hex, after[0].blocking),
        (before[0].hex, before[0].blocking)
    );
    let ((n0, top0), (n1, top1)) = (drawn(&whole, &before), drawn(&stubbed, &after));
    assert!(n1 < n0 * 2 / 3, "{n1} of {n0} pixels kept");
    assert!(top1 > top0 + 30, "top {top1} vs {top0}");
    // Taller than the kerb: a stub at the kerb's height keeps less.
    let mut at_kerb = DeclutterProfile::readable();
    {
        let c = at_kerb.cutaway.as_mut().unwrap();
        let kerb = c.kerb_height;
        c.doors.as_mut().unwrap().stub_height = kerb;
    }
    let kerbed = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &indoors(210.0, Some(at_kerb)),
    )
    .unwrap();
    let (n2, top2) = drawn(&kerbed, &of(&kerbed, SOUTH_DOOR));
    assert!(
        n2 < n1 && top2 > top1,
        "kerb {n2} px from {top2}, stub {n1} px from {top1}"
    );
    // The far door stands in a wall nobody is hidden behind: untouched.
    let (far0, far1) = (of(&whole, NORTH_DOOR), of(&stubbed, NORTH_DOOR));
    assert_eq!(whole.art[far0[0].art], stubbed.art[far1[0].art]);
    let c = &stubbed.report.declutter.as_ref().unwrap().cutaway;
    assert_eq!(c.doors_considered, 2, "{c:?}");
    assert_eq!(c.doors_trimmed, 1, "{c:?}");
    assert!(c.door_walkers_after < c.door_walkers_before, "{c:?}");
    assert!(
        stubbed
            .report
            .to_string()
            .contains("doors: 1 of 2 cut to the stub")
    );
    // Without the door rule's report the profile prints no door line.
    assert!(!whole.report.to_string().contains("cut to the stub"));
}

#[test]
fn a_teleport_door_keeps_enough_to_click() {
    let assets = room_assets();
    let mut refs = room();
    let door = refs
        .iter_mut()
        .find(|r| r.form == FormKey::new("test.esm", SOUTH_DOOR));
    door.unwrap().teleport = Some(TeleportDoc {
        door: FormKey::new("test.esm", 999),
        cell: None,
        position: [0.0; 3],
        rotation: [0.0; 3],
    });
    let mut profile = DeclutterProfile::readable();
    let stub = profile.cutaway.as_mut().unwrap().doors.as_mut().unwrap();
    stub.stub_height = 36.0;
    stub.min_click_pixels = 1_500;
    let sliced = run(
        &assets,
        &refs,
        &on_floor(),
        &big_budget(),
        &indoors(210.0, Some(profile)),
    )
    .unwrap();
    let (n, _) = drawn(&sliced, &of(&sliced, SOUTH_DOOR));
    assert!(n >= 1_500, "{n} pixels left");
    let c = &sliced.report.declutter.as_ref().unwrap().cutaway;
    assert_eq!(c.doors_grown, 1, "{c:?}");
}

/// Strips of a reference, as (hex, art) pairs, for comparing two runs.
fn strips_of(sliced: &Sliced, id: u32) -> Vec<(i32, ArtImage)> {
    of(sliced, id)
        .iter()
        .map(|p| (p.hex, sliced.art[p.art].clone()))
        .collect()
}

#[test]
fn furniture_shaped_like_a_wall_is_not_cut() {
    let assets = room_assets();
    let mut refs = room();
    // A long shelf (8 hexes, 140 high) and a full-height partition, both
    // between the camera and the walkers in the room's middle.
    refs.push(put(
        40,
        "STAT",
        "groceryshelves01",
        "shelf",
        [0.0, -250.0],
        0.0,
    ));
    refs.push(put(
        41,
        "STAT",
        "plainwall",
        "partition",
        [0.0, -120.0],
        0.0,
    ));
    let ground = on_floor();
    let config = |profile: DeclutterProfile| indoors(210.0, Some(profile));
    let uncut = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &config(DeclutterProfile {
            cutaway: None,
            ..DeclutterProfile::readable()
        }),
    )
    .unwrap();
    let mut old = DeclutterProfile::readable();
    {
        let c = old.cutaway.as_mut().unwrap();
        c.kinds.clear();
        c.exclude_patterns.clear();
        c.reach_cut = false;
        c.room_height_ratio = 0.0;
        c.doors = None;
    }
    let before = run(&assets, &refs, &ground, &big_budget(), &config(old)).unwrap();
    let after = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &config(DeclutterProfile::readable()),
    )
    .unwrap();
    // Before, the shelf was taken for a wall and cut; now it is whole.
    assert_ne!(strips_of(&before, 40), strips_of(&uncut, 40));
    assert_eq!(strips_of(&after, 40), strips_of(&uncut, 40));
    // The partition reaches the ceiling cut: still a wall.
    assert_ne!(strips_of(&after, 41), strips_of(&uncut, 41));
    let d = after.report.declutter.as_ref().unwrap();
    assert!(d.cutaway_excluded.values().sum::<usize>() >= 1, "{d:?}");
    // A shelf lower than the cut is out by the ceiling test alone, even
    // without its name; the room's walls set the room height.
    let mut by_ceiling = DeclutterProfile::readable();
    by_ceiling
        .cutaway
        .as_mut()
        .unwrap()
        .exclude_patterns
        .clear();
    let unnamed = run(&assets, &refs, &ground, &big_budget(), &config(by_ceiling)).unwrap();
    assert_eq!(strips_of(&unnamed, 40), strips_of(&uncut, 40));
    let d = unnamed.report.declutter.as_ref().unwrap();
    assert_eq!(
        d.cutaway_excluded.get("under the ceiling cut"),
        Some(&1),
        "{d:?}"
    );
    assert_eq!(d.room_wall_height, Some(200.0));
    // The per-reference opt-out keeps the partition whole too.
    let mut opt_out = DeclutterProfile::readable();
    opt_out
        .cutaway
        .as_mut()
        .unwrap()
        .exclude_refs
        .insert(key(41));
    let kept = run(&assets, &refs, &ground, &big_budget(), &config(opt_out)).unwrap();
    assert_eq!(strips_of(&kept, 41), strips_of(&uncut, 41));
    let d = kept.report.declutter.as_ref().unwrap();
    assert_eq!(d.cutaway_excluded.get("opted out"), Some(&1), "{d:?}");
}

#[test]
fn outlines_go_by_kind_whatever_the_protection() {
    let assets = room_assets();
    let mut refs = room();
    // A carried item too large to be clutter.
    refs.push(put(50, "MISC", "toolbox", "crate", [100.0, 100.0], 0.0));
    let mut profile = DeclutterProfile::readable();
    profile.protected_kinds.retain(|k| k != "MISC");
    let sliced = run(
        &assets,
        &refs,
        &on_floor(),
        &big_budget(),
        &indoors(210.0, Some(profile)),
    )
    .unwrap();
    let d = sliced.report.declutter.as_ref().unwrap();
    assert!(of(&sliced, 50).len() == 1, "the item survives pruning");
    assert_eq!(d.outlined_kinds.get("MISC"), Some(&1), "{d:?}");
    assert_eq!(d.outlined_kinds.get("DOOR"), Some(&2), "{d:?}");
}

/// A table in an open room, with every hex of the room open.
fn table_room() -> (TestAssets, Vec<Reference>) {
    let mut refs = room();
    refs.retain(|r| r.form.to_string() != key(SOUTH_DOOR) && r.form.to_string() != key(NORTH_DOOR));
    refs.push(put(60, "STAT", "pooltable", "table", [0.0, 0.0], 0.0));
    refs.push(put(61, "STAT", "crate", "crate", [300.0, 300.0], 0.0));
    (room_assets(), refs)
}

#[test]
fn furniture_gets_a_footprint_and_its_body_beats_open_hexes() {
    let (assets, refs) = table_room();
    let ground = on_floor();
    let mut plain = indoors(210.0, None);
    plain.open_hexes = inside(20.0);
    let without = run(&assets, &refs, &ground, &big_budget(), &plain).unwrap();
    assert!(without.footprints.is_empty());
    assert!(without.report.furniture.is_none());
    let with = SlicerConfig {
        furniture: Some(FurnitureRule::default()),
        ..plain.clone()
    };
    let sliced = run(&assets, &refs, &ground, &big_budget(), &with).unwrap();
    // The hexes whose centre lies under the table top.
    let table = refs.iter().find(|r| r.form.to_string() == key(60)).unwrap();
    let bounds = MeshFacts::of(&assets.mesh("table").unwrap())
        .bounds
        .unwrap();
    let footprint = Footprint::of(&table.transform, bounds);
    let plane = plane();
    let tile: Vec<[f64; 2]> = footprint
        .polygon
        .iter()
        .map(|p| plane.tile_of_source(*p))
        .collect();
    let under: BTreeSet<i32> = plane.hexes_in(&tile).into_iter().collect();
    assert!(under.len() >= 4, "{under:?}");
    let blocked = |s: &Sliced| -> BTreeSet<i32> {
        let mut b: BTreeSet<i32> = s.blocked_hexes.iter().copied().collect();
        b.extend(s.placements.iter().filter(|p| p.blocking).map(|p| p.hex));
        b
    };
    let free_before = under.difference(&blocked(&without)).count();
    let free_after = under.difference(&blocked(&sliced)).count();
    assert!(free_before > 0, "the open hexes freed the table before");
    assert_eq!(free_after, 0, "{free_after} hexes under the table are free");
    // The aisle around the table stays open: a hex two steps out.
    let aisle: Vec<i32> = with
        .open_hexes
        .iter()
        .copied()
        .filter(|&h| {
            plane
                .hex_tile(h)
                .is_some_and(|c| crate::slice::distance_to_polygon(&tile, c) > 1.0)
        })
        .collect();
    let b = blocked(&sliced);
    assert!(aisle.iter().filter(|h| b.contains(h)).count() * 20 < aisle.len());
    // Its footprint and the crate's are given for the builder, as the
    // builder takes them.
    let given = &sliced.footprints[&key(60)];
    let want: Vec<[f32; 2]> = footprint
        .polygon
        .iter()
        .map(|p| [p[0] as f32, p[1] as f32])
        .collect();
    assert_eq!(given, &want);
    assert!(sliced.footprints.contains_key(&key(61)));
    // The walls are structural: no footprint.
    assert!(!sliced.footprints.contains_key(&key(SOUTH_WALL)));
    let f = sliced.report.furniture.as_ref().unwrap();
    assert_eq!(f.footprints, sliced.footprints.len());
    let why: BTreeMap<String, String> = f
        .choices
        .iter()
        .map(|c| (c.reference.clone(), c.reason.clone()))
        .collect();
    assert_eq!(why[&key(60)], "sliced");
    assert_eq!(why[&key(61)], "scenery sprite");
    assert_eq!(why[&key(SOUTH_WALL)], "structural");
    assert!(f.open_blocked > 0);
    assert!(sliced.report.to_string().contains("Furniture:"));
    // Footprints alone, open hexes still winning.
    let footprints_only = SlicerConfig {
        furniture: Some(FurnitureRule {
            block_open: false,
            ..FurnitureRule::default()
        }),
        ..plain
    };
    let only = run(&assets, &refs, &ground, &big_budget(), &footprints_only).unwrap();
    assert_eq!(only.footprints, sliced.footprints);
    assert_eq!(only.blocked_hexes, without.blocked_hexes);
}

#[test]
fn a_roof_over_walkable_ground_leaves_the_render_outdoors() {
    let assets = TestAssets::standard();
    let ground = flat();
    let house = reference(
        1,
        "STAT",
        "meshes\\test\\house.nif",
        "house",
        &ground,
        [0.0, 0.0],
        0.0,
        0.0,
    );
    // Everyone may stand under the house (as under a canopy).
    let plane = plane();
    let facts = MeshFacts::of(&assets.mesh("house").unwrap());
    let footprint = Footprint::of(&house.transform, facts.bounds.unwrap());
    let tile: Vec<[f64; 2]> = footprint
        .polygon
        .iter()
        .map(|p| plane.tile_of_source(*p))
        .collect();
    let under: BTreeSet<i32> = plane.hexes_in(&tile).into_iter().collect();
    let base = SlicerConfig {
        open_hexes: under.clone(),
        ..SlicerConfig::default()
    };
    let profile = |roof: Option<RoofCut>| DeclutterProfile {
        roof_cut: roof,
        ..DeclutterProfile::default()
    };
    let with_roof = run(
        &assets,
        std::slice::from_ref(&house),
        &ground,
        &big_budget(),
        &SlicerConfig {
            declutter: Some(profile(None)),
            ..base.clone()
        },
    )
    .unwrap();
    let cut = run(
        &assets,
        std::slice::from_ref(&house),
        &ground,
        &big_budget(),
        &SlicerConfig {
            declutter: Some(profile(Some(RoofCut::default()))),
            ..base.clone()
        },
    )
    .unwrap();
    let (n0, top0) = drawn(&with_roof, &of(&with_roof, 1));
    let (n1, top1) = drawn(&cut, &of(&cut, 1));
    assert!(n1 < n0, "{n1} vs {n0}");
    assert!(top1 > top0, "the roof's top row left: {top1} vs {top0}");
    // The walls stay.
    assert!(n1 > n0 / 5, "{n1} vs {n0}");
    let d = cut.report.declutter.as_ref().unwrap();
    assert_eq!(d.roof_cut_references, 1);
    assert!(d.roof_cut_triangles > 0);
    // Nobody under it: nothing leaves.
    let empty = run(
        &assets,
        std::slice::from_ref(&house),
        &ground,
        &big_budget(),
        &SlicerConfig {
            declutter: Some(DeclutterProfile {
                walkers: Some(BTreeSet::new()),
                ..profile(Some(RoofCut::default()))
            }),
            ..base
        },
    )
    .unwrap();
    assert_eq!(empty.art, with_roof.art);
    assert_eq!(
        empty.report.declutter.as_ref().unwrap().roof_cut_references,
        0
    );
}

#[test]
fn a_landmark_reaching_beyond_the_view_drops_only_what_cannot_be_anchored() {
    let assets = TestAssets::standard();
    let ground = flat();
    // The tower stands by the map's top corner (hex 0 is the topmost hex
    // centre; this is six rows and nine columns in): its upper storeys
    // rise far above anything the engine's refresh visits.
    let plane = plane();
    let corner = plane.source_of_tile(plane.hex_tile(200 * 6 + 9).unwrap());
    let tower = reference(
        1,
        "STAT",
        "meshes\\test\\casinotower.nif",
        "tower",
        &ground,
        [corner[0] as f32, corner[1] as f32],
        0.0,
        0.0,
    );
    let landmarks = SlicerConfig {
        max_sliced_hexes: 40.0,
        landmark_crop_unreachable: false,
        ..SlicerConfig::default()
    };
    let refs = std::slice::from_ref(&tower);
    let flattened = run(&assets, refs, &ground, &big_budget(), &landmarks).unwrap();
    let fb = &flattened.report.landmarks.fallbacks;
    assert_eq!(fb.len(), 1, "{:?}", flattened.report.landmarks);
    assert!(
        fb[0].reason.contains("no hex near piece"),
        "{}",
        fb[0].reason
    );
    let cropped = run(
        &assets,
        refs,
        &ground,
        &big_budget(),
        &SlicerConfig {
            landmark_crop_unreachable: true,
            ..landmarks
        },
    )
    .unwrap();
    let l = &cropped.report.landmarks;
    assert!(l.fallbacks.is_empty(), "{l:?}");
    assert_eq!(l.references, 1);
    assert_eq!(l.unreachable.len(), 1);
    assert!(
        l.unreachable[0].pixels > 0 && l.unreachable[0].kept > 0,
        "{l:?}"
    );
    assert!(!of(&cropped, 1).is_empty());
    assert!(
        cropped
            .report
            .to_string()
            .contains("unreachable pixels dropped")
    );
}

/// Assets with a wire cart: an alpha-tested box whose texture is a grid of
/// one-texel wires, about a quarter opaque.
struct WireAssets {
    meshes: BTreeMap<String, StaticMesh>,
    textures: BTreeMap<String, Texture>,
}

impl Assets for WireAssets {
    fn mesh(&self, hash: &str) -> Result<StaticMesh, String> {
        self.meshes
            .get(hash)
            .cloned()
            .ok_or_else(|| hash.to_owned())
    }

    fn textures(&self) -> &dyn foundry_render::TextureSource {
        &self.textures
    }
}

fn wire_assets() -> WireAssets {
    const GRID: &str = "textures\\test\\grid.dds";
    let mut cart = mesh(&nif::fixture::box_mesh([60.0, 40.0, 50.0], GRID));
    for p in &mut cart.primitives {
        p.material.alpha = AlphaMode::Mask { cutoff: 0.5 };
        // The wires repeat across each side.
        if let Some(uv) = &mut p.uvs {
            for c in uv.iter_mut() {
                *c = [c[0] * 8.0, c[1] * 8.0];
            }
        }
    }
    let side = 256u32;
    let mut rgba = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let wire = x % 4 == 0 || y % 4 == 0;
            rgba.extend_from_slice(&[150, 150, 160, if wire { 255 } else { 0 }]);
        }
    }
    let mut solid = mesh(&nif::fixture::box_mesh([60.0, 40.0, 50.0], WALL));
    for p in &mut solid.primitives {
        p.material.alpha = AlphaMode::Mask { cutoff: 0.5 };
    }
    let opaque: Vec<u8> = [120u8, 90, 60, 255].repeat(16 * 16);
    WireAssets {
        meshes: [("cart".to_owned(), cart), ("box".to_owned(), solid)]
            .into_iter()
            .collect(),
        textures: [
            (
                GRID.to_owned(),
                Texture {
                    width: side,
                    height: side,
                    rgba,
                },
            ),
            (
                WALL.to_owned(),
                Texture {
                    width: 16,
                    height: 16,
                    rgba: opaque,
                },
            ),
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn a_wire_prop_renders_solid_or_is_dropped_and_a_cut_out_is_left_alone() {
    let assets = wire_assets();
    let ground = flat();
    let refs = vec![
        reference(
            1,
            "MSTT",
            "meshes\\test\\shoppingcart01.nif",
            "cart",
            &ground,
            [0.0, 0.0],
            0.0,
            0.4,
        ),
        reference(
            2,
            "STAT",
            "meshes\\test\\crate.nif",
            "box",
            &ground,
            [300.0, 0.0],
            0.0,
            0.4,
        ),
    ];
    let slice = |config: &SlicerConfig| {
        slice_with(
            &assets,
            &refs,
            &window(),
            &ground,
            &projection(),
            &mut renderer(),
            &palette_map(),
            &big_budget(),
            config,
        )
        .unwrap()
    };
    let plain = slice(&SlicerConfig::default());
    let wired = slice(&SlicerConfig {
        wire: Some(WireRule::default()),
        ..SlicerConfig::default()
    });
    let (n0, _) = drawn(&plain, &of(&plain, 1));
    let (n1, _) = drawn(&wired, &of(&wired, 1));
    assert!(n1 > 2 * n0, "the cart: {n0} pixels, {n1} with the rule");
    assert_eq!(wired.report.wire_props, 1);
    // The solid alpha-tested box keeps its pixels under the material's
    // cutoff: not a wire prop, drawn as before.
    assert_eq!(drawn(&plain, &of(&plain, 2)), drawn(&wired, &of(&wired, 2)));
    let dropped = slice(&SlicerConfig {
        wire: Some(WireRule {
            drop: true,
            ..WireRule::default()
        }),
        ..SlicerConfig::default()
    });
    assert!(of(&dropped, 1).is_empty());
    assert!(
        dropped
            .skipped
            .iter()
            .any(|s| s.reference == key(1) && s.reason == SkipReason::Wire)
    );
    assert!(dropped.report.to_string().contains("Wire props"));
}

#[test]
fn speedtree_models_say_why_they_are_skipped() {
    let mut tree = reference(
        1,
        "TREE",
        "trees\\treeleafless01.spt",
        "none",
        &flat(),
        [0.0, 0.0],
        0.0,
        0.0,
    );
    tree.mesh = None;
    let class = classify(&tree, None, 140.0, &SlicerConfig::default());
    assert_eq!(class, Class::Skip(SkipReason::TreeFormat));
    assert_eq!(SkipReason::TreeFormat.label(), "tree format not converted");
    // A model with no mesh otherwise is still "no mesh".
    tree.model = Some("meshes\\test\\lost.nif".into());
    let class = classify(&tree, None, 140.0, &SlicerConfig::default());
    assert_eq!(class, Class::Skip(SkipReason::NoMesh));
}

#[test]
fn flat_references_bake_a_deck_into_the_ground() {
    // A shelf stands upright; named in `flat_references`, it is baked into
    // the ground (a deck of an upper exterior level), while a door named
    // there stays a sprite.
    let shelf = put(1, "STAT", "shelf", "shelf", [0.0, 0.0], 0.0);
    let door = put(2, "DOOR", "door", "door", [300.0, 0.0], 0.0);
    let facts = MeshFacts::of(&mesh(&nif::fixture::box_mesh([700.0, 70.0, 140.0], WALL)));
    let plain = SlicerConfig::default();
    assert_ne!(
        classify(&shelf, Some(&facts), 140.0, &plain),
        Class::GroundFlat
    );
    let decks = SlicerConfig {
        flat_references: BTreeSet::from([key(1), key(2)]),
        ..SlicerConfig::default()
    };
    assert_eq!(
        classify(&shelf, Some(&facts), 140.0, &decks),
        Class::GroundFlat
    );
    assert_eq!(
        classify(&door, Some(&facts), 140.0, &decks),
        Class::Sprite(crate::Role::Door)
    );
    let refs = [shelf, door];
    let upright = run(&room_assets(), &refs, &on_floor(), &big_budget(), &plain).unwrap();
    let baked = run(&room_assets(), &refs, &on_floor(), &big_budget(), &decks).unwrap();
    assert!(!of(&upright, 1).is_empty());
    assert!(of(&baked, 1).is_empty(), "no placement of the deck");
    assert!(baked.overlays.len() > upright.overlays.len());
    assert_eq!(of(&baked, 2).len(), of(&upright, 2).len());
}
