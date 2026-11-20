//! Renders a few meshes from an extracted MIR directory to PNGs, for
//! looking at by hand: upside-down textures, inverted culling, a wrong up
//! axis and similar gross errors show at a glance.
//!
//! ```text
//! FOUNDRY_MIR=out/mir-goodsprings FOUNDRY_OUT=out/render-check \
//!     cargo run -p foundry-render --example real_meshes
//! ```
//!
//! `FOUNDRY_MESHES` may name mesh paths (comma separated, as in
//! `manifest.json`); by default the largest architecture meshes are used.
//! `FOUNDRY_BACKEND` is `cpu` (default), `gpu` or `auto`; with
//! `FOUNDRY_COMPARE=1` the GPU also renders each yaw-0 image and the
//! agreement is printed. Output goes only
//! to `FOUNDRY_OUT`, which must be an ignored directory: this example
//! writes images derived from game data.
//!
//! This reads the MIR files directly on purpose (no dependency on `mir`).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use foundry_render::{
    Backend, Calibration, GPU_TOLERANCE, GpuRenderer, Instance, Renderer, Scene, Texture,
    TextureSource, Window, renderer,
};

struct MirTextures {
    dir: PathBuf,
    hashes: BTreeMap<String, String>,
}

impl TextureSource for MirTextures {
    fn texture(&self, path: &str) -> Option<Cow<'_, Texture>> {
        let hash = self.hashes.get(path)?;
        read_png(&self.dir.join("textures").join(format!("{hash}.png"))).map(Cow::Owned)
    }
}

fn read_png(path: &Path) -> Option<Texture> {
    let file = File::open(path).ok()?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Texture {
        width: info.width,
        height: info.height,
        rgba,
    })
}

fn write_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let mir = PathBuf::from(std::env::var_os("FOUNDRY_MIR").ok_or("set FOUNDRY_MIR")?);
    let out = PathBuf::from(std::env::var_os("FOUNDRY_OUT").ok_or("set FOUNDRY_OUT")?);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(mir.join("manifest.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let hash_map = |key: &str| -> BTreeMap<String, String> {
        manifest[key]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(path, entry)| {
                        Some((path.clone(), entry["hash"].as_str()?.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let meshes = hash_map("meshes");
    let textures = MirTextures {
        dir: mir.clone(),
        hashes: hash_map("textures"),
    };

    let chosen: Vec<String> = match std::env::var("FOUNDRY_MESHES") {
        Ok(list) => list.split(',').map(|s| s.trim().to_owned()).collect(),
        Err(_) => {
            let info = &manifest["mesh_info"];
            let mut ranked: Vec<(u64, String)> = meshes
                .iter()
                .filter(|(path, _)| path.starts_with("meshes\\architecture\\"))
                .filter_map(|(path, hash)| Some((info[hash]["triangles"].as_u64()?, path.clone())))
                .collect();
            ranked.sort_by(|a, b| b.cmp(a));
            ranked.into_iter().take(8).map(|(_, p)| p).collect()
        }
    };
    let backend = match std::env::var("FOUNDRY_BACKEND").as_deref() {
        Ok("gpu") => Backend::Gpu,
        Ok("auto") => Backend::Auto,
        _ => Backend::Cpu,
    };
    // With FOUNDRY_COMPARE=1, the GPU renders each image too and the
    // agreement with the main backend is printed.
    let compare = std::env::var("FOUNDRY_COMPARE").is_ok_and(|v| v == "1");
    let mut reference = if compare {
        Some(GpuRenderer::new(&Calibration::default()).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let mut renderers: Vec<(i32, Box<dyn Renderer>)> = Vec::new();
    for yaw in [0, 1] {
        let calibration = Calibration {
            yaw_quarter_turns: yaw,
            ..Calibration::default()
        };
        renderers.push((
            yaw,
            renderer(backend, &calibration).map_err(|e| e.to_string())?,
        ));
    }

    for (index, path) in chosen.iter().enumerate() {
        let Some(hash) = meshes.get(path) else {
            eprintln!("{path}: not in the manifest");
            continue;
        };
        let bytes = std::fs::read(mir.join("meshes").join(format!("{hash}.glb")))
            .map_err(|e| e.to_string())?;
        let mesh = match nif::glb::read(&bytes) {
            Ok(mesh) => mesh,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let triangles: usize = mesh.primitives.iter().map(|p| p.indices.len() / 3).sum();
        for (yaw, turned) in &mut renderers {
            let scene = Scene {
                anchor: [0.0; 3],
                instances: vec![Instance::new(&mesh, Instance::IDENTITY, &textures)],
            };
            let start = Instant::now();
            match turned.render(&scene, Window::Fit) {
                Ok(image) => {
                    let name = format!("{index:02}_yaw{yaw}.png");
                    println!(
                        "{name}: {path}: {triangles} triangles, {} drawn, {} x {} px, {} missing \
                         textures, {:?}, {:?}",
                        image.triangles,
                        image.width,
                        image.height,
                        image.missing_textures.len(),
                        image.backend,
                        start.elapsed()
                    );
                    if image.width > 0 && image.height > 0 {
                        write_png(&out.join(name), &image.rgba, image.width, image.height)?;
                    }
                    if let (Some(gpu), 0) = (reference.as_mut(), *yaw) {
                        let other = gpu.render(&scene, Window::Fit).map_err(|e| e.to_string())?;
                        println!(
                            "    GPU agreement: {:?}",
                            GPU_TOLERANCE.compare(&image, &other)
                        );
                    }
                }
                Err(e) => eprintln!("{path}: {e}"),
            }
        }
    }
    Ok(())
}
