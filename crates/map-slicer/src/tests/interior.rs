//! Interior mode: one floor, the floor baked, the ceiling cut.

use nif::fixture::{NodeDesc, ShapeDesc, box_faces};

use super::*;
use crate::hex::Plane;
use crate::{
    BakedOverlay, Class, InteriorSlice, MeshFacts, Role, SkipReason, classify, estimate_with,
};

const FLOOR_Z: f32 = 3456.0;
const FLOOR_TEXTURE: &str = "textures\\test\\floor.dds";
const CEILING_TEXTURE: &str = "textures\\test\\ceiling.dds";

/// A kit room piece in one mesh: a 256 x 256 floor at z 0 (facing up),
/// four 16-unit thick walls 256 high and a two-sided ceiling at 256, its
/// origin at floor level in the middle.
fn room_mesh() -> StaticMesh {
    let floor = [
        [-128.0, -128.0, 0.0],
        [128.0, -128.0, 0.0],
        [128.0, 128.0, 0.0],
        [-128.0, 128.0, 0.0],
    ];
    let floor = ShapeDesc::from_faces("Floor", &[&floor], FLOOR_TEXTURE);
    let mut wall_faces = Vec::new();
    for (min, max) in [
        ([-128.0, 112.0, 0.0], [128.0, 128.0, 256.0]),
        ([-128.0, -128.0, 0.0], [128.0, -112.0, 256.0]),
        ([112.0, -112.0, 0.0], [128.0, 112.0, 256.0]),
        ([-128.0, -112.0, 0.0], [-112.0, 112.0, 256.0]),
    ] {
        wall_faces.extend(box_faces(min, max));
    }
    let faces: Vec<&[[f32; 3]]> = wall_faces.iter().map(|f| f.as_slice()).collect();
    let walls = ShapeDesc::from_faces("Walls", &faces, WALL);
    // Seen from below, counter-clockwise; two-sided, so it would show from
    // above too.
    let ceiling = [
        [-128.0, 128.0, 256.0],
        [128.0, 128.0, 256.0],
        [128.0, -128.0, 256.0],
        [-128.0, -128.0, 256.0],
    ];
    let mut ceiling = ShapeDesc::from_faces("Ceiling", &[&ceiling], CEILING_TEXTURE);
    ceiling.two_sided = true;
    mesh(
        &NodeDesc::root("Room")
            .with_shape(floor)
            .with_shape(walls)
            .with_shape(ceiling)
            .to_nif(),
    )
}

fn assets() -> TestAssets {
    TestAssets::new()
        .with("room", room_mesh())
        .with(
            "table",
            mesh(&nif::fixture::box_mesh([60.0, 60.0, 55.0], WALL)),
        )
        .with(
            "bottle",
            mesh(&nif::fixture::box_mesh([10.0, 10.0, 25.0], ROOF)),
        )
        .with(
            "lamp",
            mesh(&nif::fixture::box_mesh([30.0, 30.0, 50.0], ROOF)),
        )
        .with(
            "crate",
            mesh(&nif::fixture::box_mesh([100.0, 100.0, 50.0], WALL)),
        )
        .with("rug", mesh(&nif::fixture::quad([120.0, 80.0], ROOF)))
}

/// Terrain at the floor height: references are placed on it.
fn on_floor() -> GroundSampler {
    sampler(|_, _| FLOOR_Z)
}

fn indoors() -> SlicerConfig {
    SlicerConfig {
        interior: Some(InteriorSlice::at(FLOOR_Z)),
        ..SlicerConfig::default()
    }
}

fn room(id: u32, xy: [f32; 2], lift: f32) -> Reference {
    reference(
        id,
        "STAT",
        "dungeons\\test\\room.nif",
        "room",
        &on_floor(),
        xy,
        lift,
        0.0,
    )
}

fn plane() -> Plane {
    Plane::new(&projection(), window().x_west, window().y_north).unwrap()
}

/// Source `(x, y)` of a hex centre.
fn source_of(h: i32) -> [f64; 2] {
    let p = plane();
    p.source_of_tile(p.hex_tile(h).unwrap())
}

/// Map screen position of a source point, heights from the floor.
fn screen_of(p: [f32; 3]) -> [f64; 2] {
    let projection = projection();
    let w = window();
    let s = projection.screen(projection.tile_from_source([w.x_west, w.y_north, FLOOR_Z], p));
    [f64::from(s[0]), f64::from(s[1])]
}

fn opaque_at(overlays: &[BakedOverlay], p: [i64; 2]) -> bool {
    overlays.iter().any(|o| {
        let (x, y) = (p[0] - i64::from(o.origin[0]), p[1] - i64::from(o.origin[1]));
        x >= 0
            && y >= 0
            && (x as u32) < o.width
            && (y as u32) < o.height
            && o.rgba[(y as usize * o.width as usize + x as usize) * 4 + 3] != 0
    })
}

/// The topmost opaque screen row of the placements drawn.
fn top_row(sliced: &Sliced, placements: &[&Placement]) -> i64 {
    let mut top = i64::MAX;
    for p in placements {
        let a = &sliced.art[p.art];
        let o = frame_origin(a, p);
        for y in 0..a.height as usize {
            if (0..a.width as usize).any(|x| a.pixels[y * a.width as usize + x] != 0) {
                top = top.min(o[1] + y as i64);
                break;
            }
        }
    }
    top
}

fn strips_of<'a>(sliced: &'a Sliced, r: &Reference) -> Vec<&'a Placement> {
    sliced
        .placements
        .iter()
        .filter(|p| p.reference == r.form.to_string() && p.role == Role::WallStrip)
        .collect()
}

/// How far inside the room's outline a point is (0 on it).
fn depth_inside(xy: [f64; 2], centre: [f32; 2]) -> f64 {
    let dx = (xy[0] - f64::from(centre[0])).abs();
    let dy = (xy[1] - f64::from(centre[1])).abs();
    128.0 - dx.max(dy)
}

#[test]
fn a_room_piece_bakes_its_floor_drops_its_ceiling_and_keeps_its_walls() {
    let centre = [0.0, 0.0];
    let r = room(1, centre, 0.0);
    let refs = [r.clone()];
    // The terrain passed is ignored indoors: a slope gives the same result
    // as the floor.
    let sliced = run(&assets(), &refs, &sloped(), &big_budget(), &indoors()).unwrap();
    let flat = run(&assets(), &refs, &on_floor(), &big_budget(), &indoors()).unwrap();
    assert_eq!(sliced, flat);
    let stats = sliced.report.interior.clone().expect("interior tallies");
    assert_eq!(stats.settings, InteriorSlice::at(FLOOR_Z));

    // The floor is in the overlays, baked first.
    assert_eq!(stats.floor_triangles, 2);
    assert_eq!(stats.floor_references, 1);
    assert_eq!(stats.floor_only, 0);
    assert!(stats.floor_overlays >= 1);
    assert!((stats.floor_area - 256.0 * 256.0).abs() < 1.0);
    assert!(stats.floor_hexes.len() >= 9, "{:?}", stats.floor_hexes);
    let middle = screen_of([0.0, 0.0, FLOOR_Z]);
    assert!(opaque_at(
        &sliced.overlays[..stats.floor_overlays],
        [middle[0].round() as i64, middle[1].round() as i64]
    ));

    // The ceiling and the top of the walls are cut.
    assert_eq!(stats.clipped_references, 1);
    assert!(stats.cut_triangles >= 2 + 4 * 2, "{stats:?}");
    let strips = strips_of(&sliced, &r);
    assert!(!strips.is_empty());
    let cut_top = [-128.0f32, 128.0]
        .iter()
        .flat_map(|&x| [-128.0f32, 128.0].map(|y| screen_of([x, y, FLOOR_Z + 210.0])[1]))
        .fold(f64::INFINITY, f64::min);
    let top = top_row(&sliced, &strips);
    assert!(
        top as f64 >= cut_top - 2.0,
        "strip pixels at row {top}, above the cut's {cut_top}"
    );

    // Outdoors, the same piece shows its two-sided ceiling over the room
    // and its floor in the strips.
    let outdoors = run(
        &assets(),
        &refs,
        &on_floor(),
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(outdoors.report.interior.is_none());
    let outdoor_strips = strips_of(&outdoors, &r);
    assert!(top_row(&outdoors, &outdoor_strips) < top - 15);
    assert!(
        outdoor_strips
            .iter()
            .any(|p| depth_inside(source_of(p.hex), centre) > 60.0)
    );

    // Indoors the strips are walls only: every strip hex lies by a wall,
    // and the middle of the room is open floor.
    for p in &strips {
        let depth = depth_inside(source_of(p.hex), centre);
        assert!(
            depth < 16.0 + 50.0,
            "strip at hex {} is {depth} inside",
            p.hex
        );
    }
    let plane = plane();
    let middle_hexes = plane.hexes_in(
        &[[-40.0, -40.0], [40.0, -40.0], [40.0, 40.0], [-40.0, 40.0]]
            .map(|p| plane.tile_of_source(p)),
    );
    assert!(!middle_hexes.is_empty());
    for h in middle_hexes {
        assert!(stats.floor_hexes.contains(&h));
        assert!(sliced.blocked_hexes.binary_search(&h).is_err());
        assert!(!sliced.placements.iter().any(|p| p.hex == h && p.blocking));
    }
    assert!(strips.iter().any(|p| p.blocking));
    assert_eq!(sliced.report.strips.floor_pieces, 0);

    // The report says so.
    let text = sliced.report.to_string();
    assert!(text.contains("Interior: floor at z 3456"), "{text}");
    assert!(
        text.contains("floor baked: 2 triangles from 1 references"),
        "{text}"
    );
}

#[test]
fn things_above_the_cut_hanging_from_it_or_below_the_floor_are_skipped() {
    let floor = on_floor();
    let lamp = reference(
        2,
        "LIGH",
        "lights\\lamp.nif",
        "lamp",
        &floor,
        [0.0, 0.0],
        170.0,
        0.0,
    );
    let fan = reference(
        3,
        "STAT",
        "clutter\\fan.nif",
        "lamp",
        &floor,
        [100.0, 0.0],
        215.0,
        0.0,
    );
    let cellar = reference(
        4,
        "STAT",
        "clutter\\crate.nif",
        "crate",
        &floor,
        [0.0, 200.0],
        -300.0,
        0.0,
    );
    let table = reference(
        5,
        "STAT",
        "furniture\\table.nif",
        "table",
        &floor,
        [-200.0, 0.0],
        0.0,
        0.0,
    );
    // A tall shelf reaching above the cut stands on the floor: clipped,
    // kept.
    let shelf = reference(
        6,
        "STAT",
        "clutter\\shelf.nif",
        "lamp",
        &floor,
        [200.0, 0.0],
        0.0,
        0.0,
    );
    let mut shelf = shelf;
    shelf.transform.scale = 5.0;
    let refs = [
        lamp.clone(),
        fan.clone(),
        cellar.clone(),
        table.clone(),
        shelf.clone(),
    ];
    let sliced = run(&assets(), &refs, &floor, &big_budget(), &indoors()).unwrap();
    let reason = |r: &Reference| {
        sliced
            .skipped
            .iter()
            .find(|s| s.reference == r.form.to_string())
            .map(|s| s.reason.clone())
    };
    assert_eq!(reason(&lamp), Some(SkipReason::Hanging));
    assert_eq!(reason(&fan), Some(SkipReason::AboveCut));
    assert_eq!(reason(&cellar), Some(SkipReason::BelowFloor));
    assert_eq!(reason(&table), None);
    assert_eq!(reason(&shelf), None);
    let stats = sliced.report.interior.as_ref().unwrap();
    assert_eq!(
        (stats.above_cut, stats.hanging, stats.below_floor),
        (1, 1, 1)
    );
    assert_eq!(
        sliced.report.skips.get("hanging from the ceiling"),
        Some(&1)
    );
    assert_eq!(sliced.report.skips.get("above the ceiling cut"), Some(&1));
    assert_eq!(sliced.report.skips.get("below the floor"), Some(&1));
    for r in [&table, &shelf] {
        assert!(
            sliced
                .placements
                .iter()
                .any(|p| p.reference == r.form.to_string())
        );
    }
    assert!(stats.clipped_references >= 1);
    let text = sliced.report.to_string();
    assert!(
        text.contains("skipped 1 above the cut, 1 hanging, 1 below the floor"),
        "{text}"
    );
}

#[test]
fn furniture_and_what_stands_on_it_are_drawn_where_they_stand() {
    // A table on the floor, a bottle (an item) and a lamp on the table
    // (their origins 55 above the floor), a rug: indoors on any terrain,
    // exactly as outdoors on flat terrain at the floor's height.
    let floor = on_floor();
    let refs = [
        reference(
            10,
            "STAT",
            "furniture\\table.nif",
            "table",
            &floor,
            [30.0, 40.0],
            0.0,
            0.4,
        ),
        reference(
            11,
            "MISC",
            "clutter\\bottle.nif",
            "bottle",
            &floor,
            [38.0, 47.0],
            55.0,
            0.0,
        ),
        reference(
            12,
            "STAT",
            "clutter\\lamp.nif",
            "lamp",
            &floor,
            [20.0, 35.0],
            55.0,
            0.0,
        ),
        reference(
            13,
            "STAT",
            "rugs\\rug.nif",
            "rug",
            &floor,
            [-150.0, -60.0],
            1.0,
            0.0,
        ),
    ];
    let indoor = run(&assets(), &refs, &sloped(), &big_budget(), &indoors()).unwrap();
    let outdoor = run(
        &assets(),
        &refs,
        &floor,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert_eq!(indoor.art, outdoor.art);
    assert_eq!(indoor.placements, outdoor.placements);
    assert_eq!(indoor.overlays, outdoor.overlays);
    assert_eq!(indoor.blocked_hexes, outdoor.blocked_hexes);
    assert!(indoor.skipped.is_empty(), "{:?}", indoor.skipped);
    assert_eq!(indoor.placements.len(), 3);
    let stats = indoor.report.interior.as_ref().unwrap();
    assert_eq!(stats.floor_triangles, 0);
    assert_eq!(stats.clipped_references, 0);
    // The lamp's underside faces down above the floor band: dropped,
    // unseen from above either way.
    assert_eq!(stats.ceiling_triangles, 2);

    // Something flat lifted off the floor is not flattened onto it
    // indoors: a 25-unit bottle on the table is flat outdoors, upright
    // indoors, at the hex under it, drawn 55 units up.
    let bottle = reference(
        14,
        "STAT",
        "clutter\\bottle.nif",
        "bottle",
        &floor,
        [20.0, 35.0],
        55.0,
        0.0,
    );
    let indoor = run(
        &assets(),
        std::slice::from_ref(&bottle),
        &sloped(),
        &big_budget(),
        &indoors(),
    )
    .unwrap();
    let outdoor = run(
        &assets(),
        std::slice::from_ref(&bottle),
        &floor,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(outdoor.placements.is_empty());
    assert_eq!(outdoor.overlays.len(), 1);
    assert!(indoor.overlays.is_empty());
    let [p] = indoor.placements.as_slice() else {
        panic!("{:?}", indoor.placements)
    };
    let (g, offset, _) = crate::slice::standing(&projection(), &window(), &floor, &bottle).unwrap();
    let h = crate::hex::pick([g[0] as f64, g[1] as f64]).unwrap();
    assert_eq!(p.hex, h);
    assert!(offset[1] < -20, "{offset:?}");
    let c = crate::hex::center(h).unwrap();
    assert_eq!(
        [i64::from(p.displacement[0]), i64::from(p.displacement[1])],
        [
            g[0] + i64::from(offset[0]) - c[0],
            g[1] + i64::from(offset[1]) - c[1]
        ]
    );
}

#[test]
fn a_second_storey_is_not_drawn() {
    let ground = room(20, [0.0, 0.0], 0.0);
    let upstairs = room(21, [0.0, 0.0], 256.0);
    let refs = [ground.clone(), upstairs.clone()];
    let sliced = run(&assets(), &refs, &on_floor(), &big_budget(), &indoors()).unwrap();
    let alone = run(&assets(), &[ground], &on_floor(), &big_budget(), &indoors()).unwrap();
    assert!(
        !sliced
            .placements
            .iter()
            .any(|p| p.reference == upstairs.form.to_string())
    );
    assert_eq!(sliced.skipped.len(), 1);
    assert_eq!(sliced.skipped[0].reason, SkipReason::AboveCut);
    assert_eq!(sliced.art, alone.art);
    assert_eq!(sliced.placements, alone.placements);
    assert_eq!(sliced.overlays, alone.overlays);
    assert_eq!(sliced.report.interior.as_ref().unwrap().floor_references, 1);
}

#[test]
fn a_shell_too_large_to_slice_outdoors_is_sliced_indoors() {
    // Above `max_sliced_hexes` a mesh is a landmark outdoors; indoors it is
    // walls once its floor is baked and its ceiling cut.
    let config = SlicerConfig {
        max_sliced_hexes: 5.0,
        ..indoors()
    };
    let r = room(30, [0.0, 0.0], 0.0);
    let facts = MeshFacts::of(&room_mesh());
    let outdoors = SlicerConfig {
        interior: None,
        ..config.clone()
    };
    assert_eq!(
        classify(&r, Some(&facts), 140.0, &outdoors),
        Class::Landmark
    );
    assert_eq!(classify(&r, Some(&facts), 140.0, &config), Class::Sliced);
    let sliced = run(
        &assets(),
        std::slice::from_ref(&r),
        &on_floor(),
        &big_budget(),
        &config,
    )
    .unwrap();
    assert!(!strips_of(&sliced, &r).is_empty());
    assert_eq!(sliced.report.interior.as_ref().unwrap().floor_triangles, 2);

    // Rock is a wall indoors, not relief.
    let mut cave = r.clone();
    cave.model = Some("dungeons\\caves\\rockwall01.nif".into());
    assert_eq!(
        classify(&cave, Some(&facts), 140.0, &outdoors),
        Class::GroundRelief
    );
    assert_eq!(classify(&cave, Some(&facts), 140.0, &config), Class::Sliced);

    // The estimate counts its walls, not its floor.
    let mut big = r;
    big.transform.scale = 3.0;
    let estimate = estimate_with(
        &assets(),
        &[big.clone()],
        &window(),
        &on_floor(),
        &projection(),
        &big_budget(),
        &config,
    )
    .unwrap();
    let outdoor_estimate = estimate_with(
        &assets(),
        &[big],
        &window(),
        &on_floor(),
        &projection(),
        &big_budget(),
        &SlicerConfig {
            landmarks: false,
            max_sliced_hexes: 1.0e9,
            ..SlicerConfig::default()
        },
    )
    .unwrap();
    assert!(estimate.interior.is_some());
    assert!(estimate.strips.placed > 0);
    assert!(estimate.strips.placed < outdoor_estimate.strips.placed);
}

#[test]
fn the_budget_ladder_keeps_the_floor_bake_and_the_cut() {
    let r = room(50, [0.0, 0.0], 0.0);
    let refs = [r.clone()];
    let fits = run(&assets(), &refs, &on_floor(), &big_budget(), &indoors()).unwrap();
    let walls = fits.report.category(crate::ArtCategory::Walls).used;
    assert!(walls > 1);

    // No room anywhere: the room is demoted to relief, its walls cut, its
    // floor still baked first.
    let none = Budget {
        walls: 0,
        scenery: 0,
        items: 0,
    };
    let demoted = run(&assets(), &refs, &on_floor(), &none, &indoors()).unwrap();
    assert!(!demoted.report.ladder.is_empty());
    assert!(demoted.placements.is_empty());
    let stats = demoted.report.interior.as_ref().unwrap();
    assert_eq!(stats.floor_triangles, 2);
    assert_eq!(stats.floor_references, 1);
    assert_eq!(stats.clipped_references, 1);
    assert!(demoted.overlays.len() > stats.floor_overlays);
    let relief = &demoted.overlays[stats.floor_overlays..];
    let top = relief.iter().map(|o| i64::from(o.origin[1])).min().unwrap();
    let cut_top = [-128.0f32, 128.0]
        .iter()
        .flat_map(|&x| [-128.0f32, 128.0].map(|y| screen_of([x, y, FLOOR_Z + 210.0])[1]))
        .fold(f64::INFINITY, f64::min);
    assert!(
        top as f64 >= cut_top - 3.0,
        "relief at {top}, cut at {cut_top}"
    );
    // Its walls block the hexes under them; the middle stays open.
    assert!(!demoted.blocked_hexes.is_empty());
    let middle = crate::hex::pick({
        let s = screen_of([0.0, 0.0, FLOOR_Z]);
        [s[0], s[1]]
    })
    .unwrap();
    assert!(demoted.blocked_hexes.binary_search(&middle).is_err());

    // Room for one landmark's pieces in the scenery list but none in the
    // walls list: the room becomes a landmark, cut the same way.
    let scenery_only = Budget {
        walls: 0,
        scenery: 4000,
        items: 0,
    };
    let landmark = run(&assets(), &refs, &on_floor(), &scenery_only, &indoors()).unwrap();
    assert_eq!(landmark.report.landmarks.references, 1);
    let stats = landmark.report.interior.as_ref().unwrap();
    assert_eq!(stats.floor_triangles, 2);
    let pieces: Vec<&Placement> = landmark.placements.iter().collect();
    assert!(top_row(&landmark, &pieces) as f64 >= cut_top - 2.0);
}

#[test]
fn flat_things_lie_on_the_floor_only_when_they_are_on_it() {
    let floor = on_floor();
    let facts = MeshFacts::of(&mesh(&nif::fixture::quad([60.0, 60.0], ROOF)));
    let rug = reference(
        40,
        "STAT",
        "rugs\\rug.nif",
        "rug",
        &floor,
        [0.0, 0.0],
        1.0,
        0.0,
    );
    let papers = reference(
        41,
        "STAT",
        "office\\papers.nif",
        "rug",
        &floor,
        [0.0, 0.0],
        56.0,
        0.0,
    );
    let outdoors = SlicerConfig::default();
    assert_eq!(
        classify(&rug, Some(&facts), 140.0, &indoors()),
        Class::GroundFlat
    );
    assert_eq!(
        classify(&papers, Some(&facts), 140.0, &outdoors),
        Class::GroundFlat
    );
    assert_eq!(
        classify(&papers, Some(&facts), 140.0, &indoors()),
        Class::Sprite(Role::Scenery)
    );
}

#[test]
fn interior_settings_are_checked_and_off_by_default() {
    assert_eq!(SlicerConfig::default().interior, None);
    let mut bad = InteriorSlice::at(FLOOR_Z);
    bad.floor_band = [-16.0, 300.0];
    let config = SlicerConfig {
        interior: Some(bad),
        ..SlicerConfig::default()
    };
    assert!(matches!(
        run(&assets(), &[], &on_floor(), &big_budget(), &config),
        Err(SliceError::Config(_))
    ));
    let none = run(
        &TestAssets::standard(),
        &[],
        &flat(),
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(none.report.interior.is_none());
    assert!(!none.report.to_string().contains("Interior"));
}
