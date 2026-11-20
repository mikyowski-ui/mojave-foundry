//! Shared helpers for the integration tests: fixture meshes, procedural
//! textures, transforms and an optional PNG dump.

#![allow(dead_code)]

use std::collections::BTreeMap;

use foundry_render::{RenderOutput, Texture};
use nif::StaticMesh;

/// Source units per tile and per storey at the default calibration.
pub const TILE: f32 = 140.0;
pub const STOREY: f32 = 210.0;

/// Parses and flattens a synthetic NIF.
pub fn mesh(bytes: &[u8]) -> StaticMesh {
    let file = nif::parse(bytes).expect("fixture parses");
    nif::extract_mesh(&file).expect("fixture flattens")
}

/// A box exactly one tile by one tile by one storey, untextured, standing
/// on the ground with its south-west corner at the source origin when
/// placed with [`cube_transform`].
pub fn unit_cube() -> StaticMesh {
    mesh(&nif::fixture::box_mesh([TILE, TILE, STOREY], ""))
}

/// A one-tile box of one colour.
pub fn coloured_cube(rgba: [f32; 4]) -> StaticMesh {
    let shape = nif::fixture::box_shape("Box", [TILE, TILE, STOREY], "").with_color(rgba);
    mesh(
        &nif::fixture::NodeDesc::root("Box")
            .with_shape(shape)
            .to_nif(),
    )
}

/// A one-tile horizontal quad facing up, with `texture` as its diffuse.
pub fn quad(texture: &str) -> StaticMesh {
    mesh(&nif::fixture::quad([TILE, TILE], texture))
}

/// Translation rows.
pub fn translate(x: f32, y: f32, z: f32) -> [[f32; 4]; 3] {
    [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, z]]
}

/// Places a box or quad centred on the origin so that it covers tile
/// `s ∈ [s0, s0 + 1]`, `t ∈ [t0, t0 + 1]` with the anchor at the origin.
pub fn at_tile(s0: f32, t0: f32) -> [[f32; 4]; 3] {
    translate((s0 + 0.5) * TILE, -(t0 + 0.5) * TILE, 0.0)
}

/// The screen position of tile point `(s, t, e)`, written out from the
/// contract for checking.
pub fn screen(s: f32, t: f32, e: f32) -> [f32; 2] {
    [48.0 * s + 32.0 * t, -12.0 * s + 24.0 * t - 96.0 * e]
}

/// A texture of four quadrants: top left, top right, bottom left, bottom
/// right.
pub fn quadrants(size: u32, colours: [[u8; 4]; 4]) -> Texture {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let index = usize::from(x >= size / 2) + 2 * usize::from(y >= size / 2);
            rgba.extend_from_slice(&colours[index]);
        }
    }
    Texture {
        width: size,
        height: size,
        rgba,
    }
}

/// A procedural pattern: bands and a checker, deterministic.
pub fn pattern(size: u32, seed: u32) -> Texture {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let checker = ((x / 8 + y / 8 + seed) % 2) as u8;
            let r = (x * 255 / size.max(1)) as u8;
            let g = (y * 255 / size.max(1)) as u8;
            let b = 60 + 120 * checker;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Texture {
        width: size,
        height: size,
        rgba,
    }
}

/// A one-pixel checkerboard: black and white texels alternating.
pub fn fine_checker(size: u32) -> Texture {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let v = if (x + y) % 2 == 0 { 255 } else { 0 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Texture {
        width: size,
        height: size,
        rgba,
    }
}

/// A texture source from pairs.
pub fn source(entries: Vec<(&str, Texture)>) -> BTreeMap<String, Texture> {
    entries
        .into_iter()
        .map(|(path, texture)| (path.to_owned(), texture))
        .collect()
}

/// A subdivided box of about `6 · 2 · n²` triangles with vertex normals
/// and repeating UVs on one texture: the stand-in for a real building.
pub fn dense_box(size: [f32; 3], n: u32, texture: &str) -> StaticMesh {
    let (hx, hy, h) = (size[0] / 2.0, size[1] / 2.0, size[2]);
    let faces = nif::fixture::box_faces([-hx, -hy, 0.0], [hx, hy, h]);
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for face in &faces {
        let [a, b, _, d] = *face;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [d[0] - a[0], d[1] - a[1], d[2] - a[2]];
        let normal = {
            let c = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            [c[0] / l, c[1] / l, c[2] / l]
        };
        let base = positions.len() as u32;
        for j in 0..=n {
            for i in 0..=n {
                let (fi, fj) = (i as f32 / n as f32, j as f32 / n as f32);
                positions.push([0, 1, 2].map(|k| a[k] + u[k] * fi + v[k] * fj));
                normals.push(normal);
                uvs.push([4.0 * fi, 4.0 * (1.0 - fj)]);
            }
        }
        for j in 0..n {
            for i in 0..n {
                let p = base + j * (n + 1) + i;
                let q = p + n + 1;
                indices.extend_from_slice(&[p, p + 1, q + 1, p, q + 1, q]);
            }
        }
    }
    let primitive = nif::Primitive {
        name: "Dense".to_owned(),
        positions,
        normals: Some(normals),
        uvs: Some(uvs),
        colors: None,
        indices,
        material: nif::Material {
            textures: vec![texture.to_owned()],
            ..nif::Material::default()
        },
        skinned: false,
        skin: None,
    };
    StaticMesh {
        primitives: vec![primitive],
        ..StaticMesh::default()
    }
}

/// Writes `output` as a PNG under `$FOUNDRY_RENDER_DUMP/<name>.png` when
/// that variable is set. For looking at test images by hand.
pub fn dump(name: &str, output: &RenderOutput) {
    let Some(dir) = std::env::var_os("FOUNDRY_RENDER_DUMP") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    if std::fs::create_dir_all(&dir).is_err() || output.width == 0 || output.height == 0 {
        return;
    }
    let Ok(file) = std::fs::File::create(dir.join(format!("{name}.png"))) else {
        return;
    };
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), output.width, output.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = encoder.write_header() {
        let _ = writer.write_image_data(&output.rgba);
    }
}
