//! The CPU reference path on synthetic scenes.

mod common;

use std::time::Instant;

use common::*;
use foundry_render::{
    Backend, Calibration, CpuRenderer, Instance, NoTextures, Projection, RenderError, RenderOutput,
    Renderer, Scene, Shading, Texture, Window, renderer,
};
use nif::AlphaMode;

fn cpu() -> CpuRenderer {
    CpuRenderer::new(&Calibration::default()).expect("default calibration")
}

fn render(scene: &Scene, window: Window) -> RenderOutput {
    cpu().render(scene, window).expect("render")
}

fn scene<'a>(instances: Vec<Instance<'a>>) -> Scene<'a> {
    Scene {
        anchor: [0.0; 3],
        instances,
    }
}

fn brightness(p: [u8; 4]) -> u32 {
    u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
}

fn opaque_at(out: &RenderOutput, at: [f32; 2]) -> [u8; 4] {
    let p = out
        .pixel_at_screen(at[0], at[1])
        .expect("inside the window");
    assert_eq!(p[3], 255, "expected an opaque pixel at {at:?}");
    p
}

fn transparent_at(out: &RenderOutput, at: [f32; 2]) {
    if let Some(p) = out.pixel_at_screen(at[0], at[1]) {
        assert_eq!(p[3], 0, "expected a transparent pixel at {at:?}");
    }
}

#[test]
fn unit_cube_silhouette_and_anchor() {
    let cube = unit_cube();
    let out = render(
        &scene(vec![Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures)]),
        Window::Fit,
    );
    dump("unit_cube", &out);
    // Corners project to x ∈ [0, 80], y ∈ [-108, 24]; one pixel of padding.
    assert_eq!(out.origin, [-1, -109]);
    assert_eq!((out.width, out.height), (82, 134));
    assert_eq!(out.backend, Backend::Cpu);
    // Top, west and south faces: six triangles.
    assert_eq!(out.triangles, 6);
    // The silhouette is the hexagon (0,0) (32,24) (80,12) (80,-84)
    // (48,-108) (0,-96), area 9216 px².
    let opaque = out.opaque_pixels() as f64;
    assert!(
        (opaque - 9216.0).abs() / 9216.0 < 0.01,
        "{opaque} opaque pixels"
    );
    // Padding rows and columns are empty.
    for x in 0..out.width {
        assert_eq!(out.pixel(x, 0).map(|p| p[3]), Some(0));
        assert_eq!(out.pixel(x, out.height - 1).map(|p| p[3]), Some(0));
    }
    for y in 0..out.height {
        assert_eq!(out.pixel(0, y).map(|p| p[3]), Some(0));
        assert_eq!(out.pixel(out.width - 1, y).map(|p| p[3]), Some(0));
    }
    // Just inside each hexagon corner, and just outside each edge.
    for inside in [
        [1.5, -1.5],
        [32.0, 22.0],
        [78.5, 11.0],
        [78.5, -83.0],
        [48.0, -106.0],
        [1.5, -94.0],
    ] {
        opaque_at(&out, inside);
    }
    for outside in [
        [2.0, 20.0],
        [70.0, 22.0],
        [79.0, -100.0],
        [10.0, -105.0],
        [-0.5, -50.0],
    ] {
        transparent_at(&out, outside);
    }
    // Interior pixels carry the depth of the face they show.
    let projection = Projection::new(&Calibration::default()).expect("projection");
    let top = screen(0.5, 0.5, 1.0);
    let depth = out
        .depth_at_screen(top[0] + 0.5, top[1] + 0.5)
        .expect("inside");
    let expected = projection.depth([0.5, 0.5, 1.0]);
    // The nearest sub-sample of a pixel on a top face sits up to one pixel
    // from the probe point; depth falls by about 0.063 m per pixel downwards
    // across a horizontal surface.
    assert!((depth - expected).abs() < 0.07, "{depth} vs {expected}");
    assert_eq!(out.depth_at_screen(-0.5, -0.5), Some(f32::INFINITY));
}

#[test]
fn the_face_toward_the_key_light_is_brighter() {
    let cube = unit_cube();
    let out = render(
        &scene(vec![Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures)]),
        Window::Fit,
    );
    let top = opaque_at(&out, screen(0.5, 0.5, 1.0));
    let west = opaque_at(&out, screen(0.0, 0.5, 0.5));
    let south = opaque_at(&out, screen(0.5, 1.0, 0.5));
    // Light from the upper left and above: top, then west, then south
    // (ambient only).
    assert!(brightness(top) > brightness(west), "{top:?} {west:?}");
    assert!(brightness(west) > brightness(south), "{west:?} {south:?}");
    assert_eq!(south, [102, 102, 102, 255]); // ambient 0.4 of white
    // Unlit geometry keeps its colour.
    let flat = render(
        &scene(vec![
            Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures).with_shading(Shading::Unlit),
        ]),
        Window::Fit,
    );
    for at in [
        screen(0.5, 0.5, 1.0),
        screen(0.0, 0.5, 0.5),
        screen(0.5, 1.0, 0.5),
    ] {
        assert_eq!(opaque_at(&flat, at), [255, 255, 255, 255]);
    }
}

#[test]
fn nearer_box_wins_the_depth_test() {
    let red = coloured_cube([1.0, 0.0, 0.0, 1.0]);
    let green = coloured_cube([0.0, 1.0, 0.0, 1.0]);
    // Behind: red moved a quarter of the view direction (2, -3, -1) away,
    // which keeps its screen position, then half a tile further east so
    // that part of it shows.
    let mut behind = at_tile(1.0, -0.75);
    behind[2][3] = -0.25 * STOREY;
    let near = Instance::new(&red, at_tile(0.0, 0.0), &NoTextures).with_shading(Shading::Unlit);
    let far = Instance::new(&green, behind, &NoTextures).with_shading(Shading::Unlit);
    let a = render(&scene(vec![near, far]), Window::Fit);
    let b = render(&scene(vec![far, near]), Window::Fit);
    dump("depth_order", &a);
    assert_eq!(a.rgba, b.rgba, "draw order must not matter");
    // Red's top centre: (0.5, 0.5, 1) + 0.3 · (2, -3, -1) is inside green.
    let overlap = screen(0.5, 0.5, 1.0);
    assert_eq!(opaque_at(&a, overlap), [255, 0, 0, 255]);
    // Green's top near its east edge, in front of nothing red.
    assert_eq!(opaque_at(&a, screen(1.9, -0.3, 0.75)), [0, 255, 0, 255]);
    let projection = Projection::new(&Calibration::default()).expect("projection");
    let d = a.depth_at_screen(overlap[0], overlap[1]).expect("inside");
    let expected = projection.depth([0.5, 0.5, 1.0]);
    assert!((d - expected).abs() < 0.07, "{d} vs {expected}");
}

#[test]
fn unproject_of_rendered_depth_lies_on_the_surface() {
    let cube = unit_cube();
    let out = render(
        &scene(vec![Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures)]),
        Window::Fit,
    );
    let projection = Projection::new(&Calibration::default()).expect("projection");
    let mut checked = 0;
    for y in 0..out.height {
        for x in 0..out.width {
            let depth = out.depth[(y * out.width + x) as usize];
            if !depth.is_finite() {
                continue;
            }
            let centre = [
                (out.origin[0] + x as i32) as f32 + 0.5,
                (out.origin[1] + y as i32) as f32 + 0.5,
            ];
            let [s, t, e] = projection.unproject(centre, depth);
            let tolerance = 0.03;
            let inside = [s, t, e]
                .iter()
                .all(|&c| (-tolerance..=1.0 + tolerance).contains(&c));
            let on_face =
                (e - 1.0).abs() < tolerance || s.abs() < tolerance || (t - 1.0).abs() < tolerance;
            assert!(
                inside && on_face,
                "pixel ({x}, {y}) unprojects to {:?}",
                [s, t, e]
            );
            checked += 1;
        }
    }
    assert!(checked > 9000);
}

#[test]
fn texture_corners_land_where_they_belong() {
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let yellow = [255, 255, 0, 255];
    let textures = source(vec![(
        "textures\\quad.dds",
        quadrants(8, [red, green, blue, yellow]),
    )]);
    let mesh = quad("textures\\quad.dds");
    let out = render(
        &scene(vec![
            Instance::new(&mesh, at_tile(0.0, 0.0), &textures).with_shading(Shading::Unlit),
        ]),
        Window::Fit,
    );
    dump("texture_corners", &out);
    assert!(out.missing_textures.is_empty());
    // Image top left is the north-west corner (s = 0, t = 0); image top
    // right is north-east; bottom left south-west; bottom right south-east.
    assert_eq!(opaque_at(&out, screen(0.25, 0.25, 0.0)), red);
    assert_eq!(opaque_at(&out, screen(0.75, 0.25, 0.0)), green);
    assert_eq!(opaque_at(&out, screen(0.25, 0.75, 0.0)), blue);
    assert_eq!(opaque_at(&out, screen(0.75, 0.75, 0.0)), yellow);
}

#[test]
fn mip_levels_keep_fine_textures_from_aliasing() {
    let textures = source(vec![("fine.dds", fine_checker(1024))]);
    let mesh = quad("fine.dds");
    let out = render(
        &scene(vec![
            Instance::new(&mesh, at_tile(0.0, 0.0), &textures).with_shading(Shading::Unlit),
        ]),
        Window::Fit,
    );
    // A one-texel checker squeezed onto ~40 pixels averages to mid-grey.
    for (s, t) in [(0.3, 0.3), (0.5, 0.5), (0.7, 0.4), (0.4, 0.7)] {
        let p = opaque_at(&out, screen(s, t, 0.0));
        for c in &p[..3] {
            assert!((120..=136).contains(c), "{p:?} at ({s}, {t})");
        }
    }
}

#[test]
fn alpha_modes_cut_out_texels() {
    // Left half transparent, right half opaque. Repeat wrap blends the two
    // halves near the quad's edges, so the test samples the middle of each.
    let wide = {
        let mut rgba = Vec::new();
        for _y in 0..16 {
            for x in 0..16 {
                let a = if x < 8 { 0 } else { 255 };
                rgba.extend_from_slice(&[255, 255, 255, a]);
            }
        }
        Texture {
            width: 16,
            height: 16,
            rgba,
        }
    };
    let textures = source(vec![("half.dds", wide)]);
    let base = quad("half.dds");
    let left = screen(0.25, 0.5, 0.0);
    let right = screen(0.75, 0.5, 0.0);
    for (mode, left_visible) in [
        (AlphaMode::Opaque, true),
        (AlphaMode::Mask { cutoff: 0.5 }, false),
        (AlphaMode::Mask { cutoff: 0.0 }, true),
        (AlphaMode::Blend, false),
    ] {
        let mut mesh = base.clone();
        mesh.primitives[0].material.alpha = mode;
        let out = render(
            &scene(vec![Instance::new(&mesh, at_tile(0.0, 0.0), &textures)]),
            Window::Fit,
        );
        opaque_at(&out, right);
        if left_visible {
            opaque_at(&out, left);
        } else {
            transparent_at(&out, left);
        }
        // Alpha is only ever 0 or 255.
        assert!(
            out.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 0 || p[3] == 255)
        );
    }
}

#[test]
fn back_faces_are_culled_unless_two_sided() {
    let up = quad("");
    // Turned over: rotation by half a turn about X, so it faces down.
    let flip = [
        [1.0, 0.0, 0.0, 70.0],
        [0.0, -1.0, 0.0, -70.0],
        [0.0, 0.0, -1.0, 0.0],
    ];
    let culled = render(
        &scene(vec![Instance::new(&up, flip, &NoTextures)]),
        Window::Fit,
    );
    assert_eq!(culled.triangles, 0);
    assert_eq!((culled.width, culled.height), (0, 0));

    let mut two_sided = up.clone();
    two_sided.primitives[0].material.two_sided = true;
    let shown = render(
        &scene(vec![Instance::new(&two_sided, flip, &NoTextures)]),
        Window::Fit,
    );
    let facing_up = render(
        &scene(vec![Instance::new(&up, at_tile(0.0, 0.0), &NoTextures)]),
        Window::Fit,
    );
    assert_eq!(shown.triangles, 2);
    // Lit as if it faced the viewer: the same as the upward quad.
    assert_eq!(shown.rgba, facing_up.rgba);
}

#[test]
fn an_instance_can_draw_every_face() {
    let up = quad("");
    let flip = [
        [1.0, 0.0, 0.0, 70.0],
        [0.0, -1.0, 0.0, -70.0],
        [0.0, 0.0, -1.0, 0.0],
    ];
    let shown = render(
        &scene(vec![
            Instance::new(&up, flip, &NoTextures).with_two_sided(true),
        ]),
        Window::Fit,
    );
    let facing_up = render(
        &scene(vec![Instance::new(&up, at_tile(0.0, 0.0), &NoTextures)]),
        Window::Fit,
    );
    assert_eq!(shown.triangles, 2);
    assert_eq!(shown.rgba, facing_up.rgba);
}

#[test]
fn an_instance_cutoff_keeps_a_minified_lattice() {
    // A one-texel wire grid (one texel in four opaque): squeezed onto ~40
    // pixels its mip levels average to alpha 0.25, under the material's
    // cutoff, so it vanishes; a low cutoff of the instance keeps it solid.
    let side = 512u32;
    let mut rgba = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let wire = x % 2 == 0 && y % 2 == 0;
            rgba.extend_from_slice(&[200, 200, 200, if wire { 255 } else { 0 }]);
        }
    }
    let textures = source(vec![(
        "wire.dds",
        Texture {
            width: side,
            height: side,
            rgba,
        },
    )]);
    let mut mesh = quad("wire.dds");
    mesh.primitives[0].material.alpha = AlphaMode::Mask { cutoff: 0.5 };
    let plain = render(
        &scene(vec![Instance::new(&mesh, at_tile(0.0, 0.0), &textures)]),
        Window::Fit,
    );
    let kept = render(
        &scene(vec![
            Instance::new(&mesh, at_tile(0.0, 0.0), &textures).with_alpha_cutoff(Some(0.1)),
        ]),
        Window::Fit,
    );
    assert!(plain.opaque_pixels() < kept.opaque_pixels() / 4);
    opaque_at(&kept, screen(0.5, 0.5, 0.0));
    // The default is the material's own cutoff.
    let default = render(
        &scene(vec![
            Instance::new(&mesh, at_tile(0.0, 0.0), &textures).with_alpha_cutoff(None),
        ]),
        Window::Fit,
    );
    assert_eq!(default.rgba, plain.rgba);
}

#[test]
fn mirrored_transforms_keep_outward_faces() {
    let cube = unit_cube();
    let plain = render(
        &scene(vec![Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures)]),
        Window::Fit,
    );
    // Mirrored in X: winding flips, culling must not.
    let mirror = [
        [-1.0, 0.0, 0.0, 70.0],
        [0.0, 1.0, 0.0, -70.0],
        [0.0, 0.0, 1.0, 0.0],
    ];
    let mirrored = render(
        &scene(vec![Instance::new(&cube, mirror, &NoTextures)]),
        Window::Fit,
    );
    assert_eq!(mirrored.triangles, 6);
    assert_eq!(mirrored.rgba, plain.rgba);
}

#[test]
fn missing_textures_render_grey_and_are_listed() {
    let mesh = quad("textures\\nowhere.dds");
    let out = render(
        &scene(vec![
            Instance::new(&mesh, at_tile(0.0, 0.0), &NoTextures).with_shading(Shading::Unlit),
            Instance::new(&mesh, at_tile(1.0, 0.0), &NoTextures).with_shading(Shading::Unlit),
        ]),
        Window::Fit,
    );
    assert_eq!(out.missing_textures, ["textures\\nowhere.dds"]);
    assert_eq!(opaque_at(&out, screen(0.5, 0.5, 0.0)), [128, 128, 128, 255]);
    // A malformed image counts as missing too.
    let bad = source(vec![(
        "textures\\nowhere.dds",
        Texture {
            width: 4,
            height: 4,
            rgba: vec![0; 3],
        },
    )]);
    let out = render(
        &scene(vec![Instance::new(&mesh, at_tile(0.0, 0.0), &bad)]),
        Window::Fit,
    );
    assert_eq!(out.missing_textures.len(), 1);
}

fn building_scene_textures() -> std::collections::BTreeMap<String, Texture> {
    source(vec![
        ("walls.dds", pattern(64, 0)),
        ("roof.dds", pattern(32, 1)),
    ])
}

#[test]
fn rect_windows_tile_one_large_render() {
    let building = mesh(&nif::fixture::building(
        [700.0, 420.0, 420.0],
        "walls.dds",
        "roof.dds",
    ));
    let cube = unit_cube();
    let textures = building_scene_textures();
    let scene = Scene {
        anchor: [-300.0, 400.0, 0.0],
        instances: vec![
            Instance::new(&building, translate(0.0, 0.0, 0.0), &textures),
            Instance::new(&cube, translate(420.0, -150.0, 0.0), &NoTextures),
        ],
    };
    let full = render(&scene, Window::Fit);
    dump("building", &full);
    assert!(full.opaque_pixels() > 1000);
    let [x, y] = full.origin;
    let (w, h) = (full.width, full.height);
    let split_x = w * 3 / 7;
    let split_y = h / 3;
    let mut pieces = Vec::new();
    for (px, pw) in [(0, split_x), (split_x, w - split_x)] {
        for (py, ph) in [(0, split_y), (split_y, h - split_y)] {
            let window = Window::Rect {
                x: x + px as i32,
                y: y + py as i32,
                width: pw,
                height: ph,
            };
            pieces.push((px, py, render(&scene, window)));
        }
    }
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut depth = vec![0.0f32; (w * h) as usize];
    for (px, py, piece) in &pieces {
        assert_eq!(piece.origin, [x + *px as i32, y + *py as i32]);
        for row in 0..piece.height {
            for col in 0..piece.width {
                let src = (row * piece.width + col) as usize;
                let dst = ((py + row) * w + px + col) as usize;
                rgba[dst * 4..dst * 4 + 4].copy_from_slice(&piece.rgba[src * 4..src * 4 + 4]);
                depth[dst] = piece.depth[src];
            }
        }
    }
    assert_eq!(rgba, full.rgba);
    let bits = |d: &[f32]| d.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&depth), bits(&full.depth));
}

#[test]
fn cpu_renders_are_byte_identical() {
    let building = mesh(&nif::fixture::building(
        [560.0, 420.0, 300.0],
        "walls.dds",
        "roof.dds",
    ));
    let textures = building_scene_textures();
    let scene = scene(vec![Instance::new(
        &building,
        translate(0.0, 0.0, 0.0),
        &textures,
    )]);
    let a = render(&scene, Window::Fit);
    let b = CpuRenderer::new(&Calibration::default())
        .expect("cpu")
        .render(&scene, Window::Fit)
        .expect("render");
    assert_eq!(a.rgba, b.rgba);
    assert_eq!(
        a.depth.iter().map(|d| d.to_bits()).collect::<Vec<_>>(),
        b.depth.iter().map(|d| d.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(a, b);
}

#[test]
fn yaw_rotates_about_the_anchor() {
    let cube = unit_cube();
    let calibration = Calibration {
        yaw_quarter_turns: 2,
        ..Calibration::default()
    };
    let mut turned = CpuRenderer::new(&calibration).expect("cpu");
    let out = turned
        .render(
            &scene(vec![Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures)]),
            Window::Fit,
        )
        .expect("render");
    // Half a turn moves the cube to s, t ∈ [-1, 0]: shifted by -(80, 12).
    assert_eq!(out.origin, [-81, -121]);
    assert_eq!((out.width, out.height), (82, 134));
}

#[test]
fn absurd_windows_are_refused() {
    let cube = unit_cube();
    let one = scene(vec![Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures)]);
    let mut cpu = cpu();
    let huge = Window::Rect {
        x: 0,
        y: 0,
        width: 100_000,
        height: 10,
    };
    assert!(matches!(
        cpu.render(&one, huge),
        Err(RenderError::WindowTooLarge { .. })
    ));
    let many = Window::Rect {
        x: 0,
        y: 0,
        width: 4000,
        height: 4000,
    };
    assert!(matches!(
        cpu.render(&one, many),
        Err(RenderError::WindowTooLarge { .. })
    ));
    let empty = Window::Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 10,
    };
    assert!(matches!(
        cpu.render(&one, empty),
        Err(RenderError::EmptyWindow { .. })
    ));
    let far = Window::Rect {
        x: i32::MAX - 10,
        y: 0,
        width: 4,
        height: 4,
    };
    assert!(matches!(
        cpu.render(&one, far),
        Err(RenderError::WindowOutOfRange { .. })
    ));
    // Two cubes 300 tiles apart do not fit in one window.
    let spread = scene(vec![
        Instance::new(&cube, at_tile(0.0, 0.0), &NoTextures),
        Instance::new(&cube, at_tile(300.0, 300.0), &NoTextures),
    ]);
    assert!(matches!(
        cpu.render(&spread, Window::Fit),
        Err(RenderError::WindowTooLarge { .. })
    ));
}

#[test]
fn bad_geometry_is_skipped_not_fatal() {
    let mut mesh = unit_cube();
    mesh.primitives[0].positions[0] = [f32::NAN, 0.0, 0.0];
    let mut broken = unit_cube();
    broken.primitives[0].indices.push(9999);
    let mut far = unit_cube();
    far.primitives[0].positions[1] = [1.0e30, 0.0, 0.0];
    let singular = [[0.0; 4]; 3];
    let cube = unit_cube();
    let out = render(
        &scene(vec![
            Instance::new(&mesh, at_tile(0.0, 0.0), &NoTextures),
            Instance::new(&broken, at_tile(0.0, 0.0), &NoTextures),
            Instance::new(&far, at_tile(0.0, 0.0), &NoTextures),
            Instance::new(&cube, singular, &NoTextures),
        ]),
        Window::Fit,
    );
    // The two triangles on the NaN vertex, the whole malformed primitive
    // (12), the one triangle on the far vertex and the singular instance.
    assert_eq!(out.skipped_triangles, 2 + 12 + 1 + 12);
    assert!(out.opaque_pixels() > 0);
}

#[test]
fn backend_choice() {
    let calibration = Calibration::default();
    let cpu = renderer(Backend::Cpu, &calibration).expect("cpu");
    assert_eq!(cpu.backend(), Backend::Cpu);
    let auto =
        renderer(Backend::Auto, &calibration).expect("auto never fails on a valid calibration");
    assert_ne!(auto.backend(), Backend::Auto);
    let bad = Calibration {
        supersample: 0,
        ..calibration
    };
    assert!(renderer(Backend::Auto, &bad).is_err());
    assert!(renderer(Backend::Cpu, &bad).is_err());
}

#[test]
fn hundred_thousand_triangles_render_quickly() {
    let textures = source(vec![("dense.dds", pattern(1024, 2))]);
    let mesh = dense_box([1400.0, 1120.0, 420.0], 91, "dense.dds");
    let triangles: usize = mesh.primitives.iter().map(|p| p.indices.len() / 3).sum();
    assert!(triangles >= 99_000, "{triangles}");
    let scene = scene(vec![Instance::new(
        &mesh,
        translate(0.0, 0.0, 0.0),
        &textures,
    )]);
    let mut cpu = cpu();
    let start = Instant::now();
    let out = cpu.render(&scene, Window::Fit).expect("render");
    let elapsed = start.elapsed();
    dump("dense", &out);
    eprintln!(
        "{triangles} triangles, {} x {} px at 4x supersampling: {elapsed:?} on the CPU path",
        out.width, out.height
    );
    assert!(out.triangles > 40_000);
    // Generous for a loaded build machine; typical runs are far below.
    assert!(elapsed.as_secs_f64() < 3.0, "{elapsed:?}");
}
