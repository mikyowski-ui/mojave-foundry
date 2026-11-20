//! The GPU path against the CPU reference.
//!
//! Every test skips with a printed reason when no adapter is available,
//! unless `MOJAVE_REQUIRE_GPU=1`, in which case it fails.

mod common;

use common::*;
use foundry_render::{
    Backend, Calibration, CpuRenderer, GPU_TOLERANCE, GpuRenderer, Instance, NoTextures,
    RenderOutput, Renderer, Scene, Shading, Window,
};

fn gpu(calibration: &Calibration) -> Option<GpuRenderer> {
    match GpuRenderer::new(calibration) {
        Ok(gpu) => {
            eprintln!("GPU adapter: {}", gpu.adapter());
            Some(gpu)
        }
        Err(e) => {
            if std::env::var("MOJAVE_REQUIRE_GPU").is_ok_and(|v| v == "1") {
                panic!("MOJAVE_REQUIRE_GPU=1 but no GPU renderer: {e}");
            }
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

fn agree(name: &str, cpu: &RenderOutput, gpu: &RenderOutput) {
    dump(&format!("{name}_cpu"), cpu);
    dump(&format!("{name}_gpu"), gpu);
    assert_eq!(cpu.backend, Backend::Cpu);
    assert_eq!(gpu.backend, Backend::Gpu);
    assert_eq!(cpu.missing_textures, gpu.missing_textures);
    assert_eq!(cpu.triangles, gpu.triangles);
    let agreement = GPU_TOLERANCE.compare(cpu, gpu).expect("same window");
    eprintln!(
        "{name}: {} x {} px, {} opaque; silhouette differs on {:.4} % of pixels, \
         colour outliers (> {} / 255) {:.4} %, max colour difference {}, max depth difference {:.5} m",
        cpu.width,
        cpu.height,
        cpu.opaque_pixels(),
        100.0 * agreement.silhouette_fraction,
        GPU_TOLERANCE.colour_levels,
        100.0 * agreement.colour_outlier_fraction,
        agreement.max_colour_difference,
        agreement.max_depth_difference,
    );
    assert!(GPU_TOLERANCE.accepts(&agreement), "{name}: {agreement:?}");
    assert!(
        agreement.max_depth_difference < 0.1,
        "{name}: {agreement:?}"
    );
}

#[test]
fn gpu_matches_cpu_on_a_textured_building() {
    let calibration = Calibration::default();
    let Some(mut gpu) = gpu(&calibration) else {
        return;
    };
    let mut cpu = CpuRenderer::new(&calibration).expect("cpu");
    let building = mesh(&nif::fixture::building(
        [700.0, 420.0, 420.0],
        "walls.dds",
        "roof.dds",
    ));
    let cube = unit_cube();
    let tinted = coloured_cube([0.9, 0.5, 0.2, 1.0]);
    let quad_mesh = quad("missing.dds");
    let textures = source(vec![
        ("walls.dds", pattern(64, 0)),
        ("roof.dds", pattern(256, 1)),
    ]);
    let scene = Scene {
        anchor: [-300.0, 400.0, 0.0],
        instances: vec![
            Instance::new(&building, translate(0.0, 0.0, 0.0), &textures),
            Instance::new(&cube, translate(420.0, -150.0, 0.0), &NoTextures),
            Instance::new(&tinted, translate(-500.0, 100.0, 0.0), &NoTextures)
                .with_shading(Shading::Unlit),
            Instance::new(&quad_mesh, translate(-200.0, -350.0, 0.0), &NoTextures),
        ],
    };
    let c = cpu.render(&scene, Window::Fit).expect("cpu render");
    let g = gpu.render(&scene, Window::Fit).expect("gpu render");
    agree("building", &c, &g);
}

#[test]
fn gpu_matches_cpu_on_alpha_cutouts_and_mips() {
    let calibration = Calibration::default();
    let Some(mut gpu) = gpu(&calibration) else {
        return;
    };
    let mut cpu = CpuRenderer::new(&calibration).expect("cpu");
    let mut cutout = quad("leaf.dds");
    cutout.primitives[0].material.alpha = nif::AlphaMode::Mask { cutoff: 0.5 };
    cutout.primitives[0].material.two_sided = true;
    let fine = quad("fine.dds");
    let leaf = {
        let mut t = pattern(64, 3);
        for (i, p) in t.rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % 64, i / 64);
            let dx = x as i32 - 32;
            let dy = y as i32 - 32;
            p[3] = if dx * dx + dy * dy < 26 * 26 { 255 } else { 0 };
        }
        t
    };
    let textures = source(vec![("leaf.dds", leaf), ("fine.dds", fine_checker(512))]);
    // A standing cut-out, turned to face south-west, and a fine texture.
    let (c45, s45) = (
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    );
    let standing = [
        [3.0 * c45, 0.0, -3.0 * s45, 100.0],
        [-3.0 * s45, 0.0, -3.0 * c45, -100.0],
        [0.0, 3.0, 0.0, 200.0],
    ];
    let scene = Scene {
        anchor: [0.0; 3],
        instances: vec![
            Instance::new(&cutout, standing, &textures),
            Instance::new(&fine, at_tile(2.0, 1.0), &textures),
        ],
    };
    let c = cpu.render(&scene, Window::Fit).expect("cpu render");
    let g = gpu.render(&scene, Window::Fit).expect("gpu render");
    agree("cutouts", &c, &g);
}

#[test]
fn gpu_tiles_large_windows() {
    let calibration = Calibration::default();
    let Some(mut gpu) = gpu(&calibration) else {
        return;
    };
    let mut cpu = CpuRenderer::new(&calibration).expect("cpu");
    let textures = source(vec![("dense.dds", pattern(1024, 2))]);
    let dense = dense_box([1400.0, 1120.0, 420.0], 40, "dense.dds");
    let scene = Scene {
        anchor: [0.0; 3],
        instances: vec![Instance::new(&dense, translate(0.0, 0.0, 0.0), &textures)],
    };
    // 700 px at 4 × supersampling is 2800 sub-samples: two GPU tiles wide.
    let window = Window::Rect {
        x: -320,
        y: -260,
        width: 700,
        height: 520,
    };
    let c = cpu.render(&scene, window).expect("cpu render");
    let g = gpu.render(&scene, window).expect("gpu render");
    assert!(c.opaque_pixels() > 100_000);
    agree("tiled", &c, &g);
}

#[test]
fn gpu_handles_empty_scenes() {
    let calibration = Calibration::default();
    let Some(mut gpu) = gpu(&calibration) else {
        return;
    };
    let empty = Scene::default();
    let out = gpu.render(&empty, Window::Fit).expect("render");
    assert_eq!((out.width, out.height), (0, 0));
    let rect = Window::Rect {
        x: 5,
        y: 5,
        width: 3,
        height: 2,
    };
    let out = gpu.render(&empty, rect).expect("render");
    assert_eq!(out.rgba, vec![0; 24]);
    assert!(out.depth.iter().all(|d| d.is_infinite()));
}
