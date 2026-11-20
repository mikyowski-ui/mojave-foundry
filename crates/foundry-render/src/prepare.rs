//! The step both backends share: instances to screen-space triangles.
//!
//! Each triangle is transformed to screen pixels and depth, snapped to the
//! fixed-point sub-sample grid, culled, given world normals, a texture and a
//! mip level. Vertices are expanded per triangle (three per triangle), so
//! per-triangle choices such as a flipped back-face normal live in the
//! vertices themselves.

use std::collections::BTreeMap;

use nif::{AlphaMode, Primitive};

use crate::projection::{cofactor, det3};
use crate::texture::{MipChain, Texture};
use crate::{Calibration, Projection, RenderError, Scene, Shading, Window};

/// Largest accepted window side in pixels.
pub const MAX_WINDOW_SIDE: u32 = 16384;
/// Largest accepted number of sub-samples in one render (`width · height ·
/// supersample²`); about 128 MiB of sample buffers.
pub const MAX_SAMPLES: u64 = 1 << 24;
/// Fixed-point steps per sub-sample.
pub(crate) const SUBSTEPS: i64 = 16;
/// Vertices farther than this from the anchor, in screen pixels, make their
/// triangle unusable. Keeps every fixed-point product inside `i64`.
const COORD_LIMIT: f64 = (1 << 20) as f64;

/// Texture id of the built-in white texel (untextured primitives).
pub(crate) const WHITE: u32 = 0;
/// Texture id of the built-in mid-grey texel (missing textures).
pub(crate) const GREY: u32 = 1;

/// A screen-space vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Vertex {
    /// Absolute fixed-point position: screen pixels · supersample ·
    /// [`SUBSTEPS`], rounded.
    pub fixed: [i64; 2],
    /// Depth in metres along the view direction.
    pub depth: f32,
    pub uv: [f32; 2],
    /// Linear 0..1 RGBA, opacity folded into alpha.
    pub color: [f32; 4],
    /// Unit normal in metric map axes `(s, t, e)`, facing the viewer for a
    /// visible back face.
    pub normal: [f32; 3],
}

/// Per-triangle state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Tri {
    pub texture: u32,
    pub level: u32,
    /// Alpha test cutoff; negative for opaque.
    pub cutoff: f32,
    pub lit: bool,
}

/// A window in output pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The prepared scene.
pub(crate) struct Prepared {
    pub rect: PixelRect,
    /// Three per triangle.
    pub vertices: Vec<Vertex>,
    pub tris: Vec<Tri>,
    /// Indexed by [`Tri::texture`]; `WHITE` and `GREY` first.
    pub textures: Vec<MipChain>,
    pub missing: Vec<String>,
    pub skipped: u32,
}

/// A transformed source vertex, before triangle assembly.
#[derive(Clone, Copy)]
struct Projected {
    screen: [f64; 2],
    fixed: [i64; 2],
    depth: f32,
    normal: Option<[f32; 3]>,
    valid: bool,
}

fn mul34(a: &[[f64; 3]; 3], m: &[[f64; 4]; 3]) -> [[f64; 4]; 3] {
    let mut out = [[0.0; 4]; 3];
    for r in 0..3 {
        for c in 0..4 {
            out[r][c] = a[r][0] * m[0][c] + a[r][1] * m[1][c] + a[r][2] * m[2][c];
        }
    }
    out
}

fn normalize(v: [f64; 3]) -> Option<[f32; 3]> {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (length.is_finite() && length > 1.0e-30).then(|| {
        [
            (v[0] / length) as f32,
            (v[1] / length) as f32,
            (v[2] / length) as f32,
        ]
    })
}

fn apply3(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// Checks a window against the size limits.
pub(crate) fn check_window(width: u64, height: u64, supersample: u32) -> Result<(), RenderError> {
    let samples = width
        .saturating_mul(height)
        .saturating_mul(u64::from(supersample) * u64::from(supersample));
    if width > u64::from(MAX_WINDOW_SIDE)
        || height > u64::from(MAX_WINDOW_SIDE)
        || samples > MAX_SAMPLES
    {
        return Err(RenderError::WindowTooLarge {
            width,
            height,
            supersample,
            max_side: MAX_WINDOW_SIDE,
            max_samples: MAX_SAMPLES,
        });
    }
    Ok(())
}

struct TextureTable {
    chains: Vec<MipChain>,
    ids: BTreeMap<(usize, String), u32>,
    missing: Vec<String>,
}

impl TextureTable {
    fn new() -> Self {
        TextureTable {
            chains: vec![
                MipChain::new(&Texture::solid([255, 255, 255, 255])),
                MipChain::new(&Texture::solid([128, 128, 128, 255])),
            ],
            ids: BTreeMap::new(),
            missing: Vec::new(),
        }
    }

    fn id(&mut self, source: &dyn crate::TextureSource, primitive: &Primitive) -> u32 {
        let Some(path) = primitive
            .material
            .textures
            .first()
            .filter(|p| !p.is_empty())
        else {
            return WHITE;
        };
        // Sources are told apart by address within one render only; the key
        // never reaches an output.
        let key = (
            std::ptr::from_ref(source).cast::<()>() as usize,
            path.clone(),
        );
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let id = match source.texture(path) {
            Some(texture) if texture.is_valid() => {
                self.chains.push(MipChain::new(&texture));
                (self.chains.len() - 1) as u32
            }
            _ => {
                self.missing.push(path.clone());
                GREY
            }
        };
        self.ids.insert(key, id);
        id
    }
}

/// Prepares `scene` for `window`.
pub(crate) fn prepare(
    calibration: &Calibration,
    projection: &Projection,
    scene: &Scene,
    window: Window,
) -> Result<Prepared, RenderError> {
    let ss = calibration.supersample;
    if let Window::Rect {
        x,
        y,
        width,
        height,
    } = window
    {
        if width == 0 || height == 0 {
            return Err(RenderError::EmptyWindow { width, height });
        }
        check_window(u64::from(width), u64::from(height), ss)?;
        let limit = 2 * COORD_LIMIT as i64;
        if i64::from(x).abs() > limit || i64::from(y).abs() > limit {
            return Err(RenderError::WindowOutOfRange {
                x: x.into(),
                y: y.into(),
                limit,
            });
        }
    }
    let fixed_scale = f64::from(ss) * SUBSTEPS as f64;
    let metric = projection.metric();
    let forward = projection.forward();
    let (to_tile, tile_offset) = projection.source_to_tile(scene.anchor);
    let winding = projection.winding_sign();
    let sample_area = f64::from(ss) * f64::from(ss);

    let mut textures = TextureTable::new();
    let mut vertices = Vec::new();
    let mut tris = Vec::new();
    let mut skipped = 0u32;
    let mut scratch: Vec<Projected> = Vec::new();
    // Bounds of the snapped vertices, in fixed-point steps.
    let mut bounds = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];

    for instance in &scene.instances {
        let t = instance.transform.map(|row| row.map(f64::from));
        let triangle_count = |p: &Primitive| (p.indices.len() / 3) as u32;
        if t.iter().flatten().any(|v| !v.is_finite()) {
            skipped += instance
                .mesh
                .primitives
                .iter()
                .map(triangle_count)
                .sum::<u32>();
            continue;
        }
        // Mesh-local to tile space.
        let mut tile = mul34(&to_tile, &t);
        for (row, offset) in tile.iter_mut().zip(tile_offset) {
            row[3] += offset;
        }
        let tile_linear = [0, 1, 2].map(|r| [tile[r][0], tile[r][1], tile[r][2]]);
        let det = det3(&tile_linear);
        if !(det.is_finite() && det.abs() > 1.0e-18) {
            skipped += instance
                .mesh
                .primitives
                .iter()
                .map(triangle_count)
                .sum::<u32>();
            continue;
        }
        // Mesh-local to (screen x, screen y, depth).
        let clip = mul34(forward, &tile);
        let orient = det.signum() * winding;
        // Normals: the cofactor of the metric map, sign-corrected so that
        // outward stays outward under a mirroring transform.
        let metric_linear = [0, 1, 2].map(|r| tile_linear[r].map(|v| v * metric[r]));
        let normal_matrix = cofactor(&metric_linear).map(|row| row.map(|v| v * det.signum()));
        let lit = instance.shading == Shading::Lit;

        for primitive in &instance.mesh.primitives {
            if !primitive.is_well_formed() {
                skipped += triangle_count(primitive);
                continue;
            }
            let texture = textures.id(instance.textures, primitive);
            let chain = &textures.chains[texture as usize];
            let texel_area = f64::from(chain.levels[0].width) * f64::from(chain.levels[0].height);
            let material = &primitive.material;
            let cutoff = match material.alpha {
                AlphaMode::Opaque => -1.0,
                AlphaMode::Mask { cutoff } if cutoff.is_finite() => cutoff.clamp(0.0, 1.0),
                AlphaMode::Mask { .. } | AlphaMode::Blend => 0.5,
            };
            let opacity = if material.opacity.is_finite() {
                material.opacity.clamp(0.0, 1.0)
            } else {
                1.0
            };

            scratch.clear();
            for (i, p) in primitive.positions.iter().enumerate() {
                let local = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
                let at = |r: usize| {
                    clip[r][0] * local[0]
                        + clip[r][1] * local[1]
                        + clip[r][2] * local[2]
                        + clip[r][3]
                };
                let screen = [at(0), at(1)];
                let depth = at(2);
                let valid = screen
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= COORD_LIMIT)
                    && depth.is_finite()
                    && (depth as f32).is_finite();
                let fixed = if valid {
                    [
                        (screen[0] * fixed_scale).round() as i64,
                        (screen[1] * fixed_scale).round() as i64,
                    ]
                } else {
                    [0, 0]
                };
                let normal = primitive.normals.as_ref().and_then(|n| {
                    let n = n[i].map(f64::from);
                    normalize(apply3(&normal_matrix, n))
                });
                scratch.push(Projected {
                    screen,
                    fixed,
                    depth: depth as f32,
                    normal,
                    valid,
                });
            }

            for triangle in primitive.indices.as_chunks::<3>().0 {
                let idx = [
                    triangle[0] as usize,
                    triangle[1] as usize,
                    triangle[2] as usize,
                ];
                let v = idx.map(|i| scratch[i]);
                if v.iter().any(|p| !p.valid) {
                    skipped += 1;
                    continue;
                }
                let cross = {
                    let (a, b, c) = (v[0].fixed, v[1].fixed, v[2].fixed);
                    i128::from(b[0] - a[0]) * i128::from(c[1] - a[1])
                        - i128::from(b[1] - a[1]) * i128::from(c[0] - a[0])
                };
                if cross == 0 {
                    continue;
                }
                let front = (cross as f64) * orient < 0.0;
                if !front && !material.two_sided {
                    continue;
                }
                let facing = if front { 1.0 } else { -1.0 };
                let face_normal = {
                    let p = idx.map(|i| primitive.positions[i].map(f64::from));
                    let u = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                    let w = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                    let n = [
                        u[1] * w[2] - u[2] * w[1],
                        u[2] * w[0] - u[0] * w[2],
                        u[0] * w[1] - u[1] * w[0],
                    ];
                    normalize(apply3(&normal_matrix, n)).unwrap_or([0.0, 0.0, 1.0])
                };
                let uvs = primitive
                    .uvs
                    .as_ref()
                    .map(|uv| idx.map(|i| uv[i].map(|c| if c.is_finite() { c } else { 0.0 })));
                let level = match &uvs {
                    Some(uv) => {
                        let uv_cross = (f64::from(uv[1][0]) - f64::from(uv[0][0]))
                            * (f64::from(uv[2][1]) - f64::from(uv[0][1]))
                            - (f64::from(uv[1][1]) - f64::from(uv[0][1]))
                                * (f64::from(uv[2][0]) - f64::from(uv[0][0]));
                        let screen_cross = (v[1].screen[0] - v[0].screen[0])
                            * (v[2].screen[1] - v[0].screen[1])
                            - (v[1].screen[1] - v[0].screen[1]) * (v[2].screen[0] - v[0].screen[0]);
                        let ratio = (uv_cross.abs() * texel_area
                            / (screen_cross.abs() * sample_area))
                            .sqrt();
                        chain.level_for(ratio)
                    }
                    None => 0,
                };
                for (corner, &i) in idx.iter().enumerate() {
                    let n = v[corner].normal.unwrap_or(face_normal).map(|c| c * facing);
                    let color = primitive.colors.as_ref().map_or([1.0; 4], |c| {
                        c[i].map(|x| {
                            if x.is_finite() {
                                x.clamp(0.0, 1.0)
                            } else {
                                1.0
                            }
                        })
                    });
                    let color = [color[0], color[1], color[2], color[3] * opacity];
                    vertices.push(Vertex {
                        fixed: v[corner].fixed,
                        depth: v[corner].depth,
                        uv: uvs.map_or([0.0; 2], |uv| uv[corner]),
                        color,
                        normal: n,
                    });
                    let s = v[corner].fixed;
                    bounds = [
                        bounds[0].min(s[0]),
                        bounds[1].min(s[1]),
                        bounds[2].max(s[0]),
                        bounds[3].max(s[1]),
                    ];
                }
                tris.push(Tri {
                    texture,
                    level,
                    cutoff,
                    lit,
                });
            }
        }
    }

    let rect = match window {
        Window::Rect {
            x,
            y,
            width,
            height,
        } => PixelRect {
            x,
            y,
            width,
            height,
        },
        Window::Fit if tris.is_empty() => PixelRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        Window::Fit => {
            let step = i64::from(ss) * SUBSTEPS;
            let x0 = bounds[0].div_euclid(step) - 1;
            let y0 = bounds[1].div_euclid(step) - 1;
            let x1 = (bounds[2] + step - 1).div_euclid(step) + 1;
            let y1 = (bounds[3] + step - 1).div_euclid(step) + 1;
            let (width, height) = ((x1 - x0) as u64, (y1 - y0) as u64);
            check_window(width, height, ss)?;
            PixelRect {
                x: x0 as i32,
                y: y0 as i32,
                width: width as u32,
                height: height as u32,
            }
        }
    };
    let mut missing = textures.missing;
    missing.sort();
    missing.dedup();
    Ok(Prepared {
        rect,
        vertices,
        tris,
        textures: textures.chains,
        missing,
        skipped,
    })
}
