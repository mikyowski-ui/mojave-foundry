//! The reference software rasterizer.
//!
//! Vertices are snapped to a fixed-point grid of [`SUBSTEPS`] steps per
//! sub-sample in *absolute* screen coordinates. Edge functions are exact
//! integers, sub-sample centres sit at half steps, and ties go by the
//! top-left rule, so a sub-sample's coverage and attributes do not depend
//! on the window being rendered: adjacent `Window::Rect` renders tile one
//! large render exactly. Attributes are interpolated affinely (parallel
//! projection has no perspective).

use crate::prepare::{PixelRect, Prepared, SUBSTEPS, Tri, Vertex, prepare};
use crate::resolve::{ResolveSettings, Samples, resolve};
use crate::texture::MipChain;
use crate::{Backend, Calibration, Projection, RenderError, RenderOutput, Renderer, Scene, Window};

/// The reference path: deterministic everywhere, byte for byte.
#[derive(Debug, Clone)]
pub struct CpuRenderer {
    calibration: Calibration,
    projection: Projection,
}

impl CpuRenderer {
    /// A CPU renderer for a validated calibration.
    pub fn new(calibration: &Calibration) -> Result<Self, RenderError> {
        Ok(CpuRenderer {
            calibration: calibration.clone(),
            projection: Projection::new(calibration)?,
        })
    }

    /// The projection in use.
    pub fn projection(&self) -> &Projection {
        &self.projection
    }
}

impl Renderer for CpuRenderer {
    fn render(&mut self, scene: &Scene, window: Window) -> Result<RenderOutput, RenderError> {
        let prepared = prepare(&self.calibration, &self.projection, scene, window)?;
        let samples = rasterize(
            &prepared,
            &Shade::new(&self.calibration),
            self.calibration.supersample,
        );
        Ok(finish(&self.calibration, prepared, &samples, Backend::Cpu))
    }

    fn backend(&self) -> Backend {
        Backend::Cpu
    }
}

/// Builds the output from resolved samples; shared with the GPU path.
pub(crate) fn finish(
    calibration: &Calibration,
    prepared: Prepared,
    samples: &Samples,
    backend: Backend,
) -> RenderOutput {
    let (rgba, depth) = resolve(
        samples,
        ResolveSettings {
            supersample: calibration.supersample,
            alpha_threshold: calibration.alpha_threshold,
            contrast: calibration.contrast,
        },
    );
    let PixelRect {
        x,
        y,
        width,
        height,
    } = prepared.rect;
    RenderOutput {
        origin: [x, y],
        width,
        height,
        rgba,
        depth,
        backend,
        missing_textures: prepared.missing,
        triangles: prepared.tris.len() as u32,
        skipped_triangles: prepared.skipped,
    }
}

/// The lighting constants.
pub(crate) struct Shade {
    pub light: [f32; 3],
    pub intensity: f32,
    pub ambient: f32,
}

impl Shade {
    pub fn new(calibration: &Calibration) -> Self {
        Shade {
            light: calibration.light(),
            intensity: calibration.key_light_intensity,
            ambient: calibration.ambient,
        }
    }

    fn factor(&self, n: [f32; 3]) -> f32 {
        let length2 = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
        let inv = 1.0 / length2.max(1.0e-20).sqrt();
        let lambert = (n[0] * self.light[0] + n[1] * self.light[1] + n[2] * self.light[2]) * inv;
        self.ambient + self.intensity * lambert.max(0.0)
    }
}

fn to_byte(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Edge `a → b` owns sub-samples exactly on it when it is a top or left
/// edge of a triangle with positive area (y down).
fn owns(a: [i64; 2], b: [i64; 2]) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    dy < 0 || (dy == 0 && dx > 0)
}

/// Rasterizes every prepared triangle into a sample buffer.
pub(crate) fn rasterize(prepared: &Prepared, shade: &Shade, supersample: u32) -> Samples {
    let ss = i64::from(supersample);
    let rect = prepared.rect;
    let (ws, hs) = (
        rect.width as usize * ss as usize,
        rect.height as usize * ss as usize,
    );
    let mut samples = Samples::new(ws, hs);
    if ws == 0 || hs == 0 {
        return samples;
    }
    let origin = [
        i64::from(rect.x) * ss * SUBSTEPS,
        i64::from(rect.y) * ss * SUBSTEPS,
    ];
    for (t, tri) in prepared.tris.iter().enumerate() {
        let v = &prepared.vertices[3 * t..3 * t + 3];
        draw(
            &mut samples,
            origin,
            [&v[0], &v[1], &v[2]],
            tri,
            &prepared.textures[tri.texture as usize],
            shade,
        );
    }
    samples
}

fn draw(
    samples: &mut Samples,
    origin: [i64; 2],
    v: [&Vertex; 3],
    tri: &Tri,
    texture: &MipChain,
    shade: &Shade,
) {
    let mut v = v;
    let mut p = v.map(|v| [v.fixed[0] - origin[0], v.fixed[1] - origin[1]]);
    let mut area =
        (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
    if area == 0 {
        return;
    }
    if area < 0 {
        v.swap(1, 2);
        p.swap(1, 2);
        area = -area;
    }
    let half = SUBSTEPS / 2;
    let (ws, hs) = (samples.width as i64, samples.height as i64);
    let min_x = p[0][0].min(p[1][0]).min(p[2][0]);
    let max_x = p[0][0].max(p[1][0]).max(p[2][0]);
    let min_y = p[0][1].min(p[1][1]).min(p[2][1]);
    let max_y = p[0][1].max(p[1][1]).max(p[2][1]);
    // Sub-sample i has its centre at i · SUBSTEPS + half.
    let i0 = (min_x - half + SUBSTEPS - 1).div_euclid(SUBSTEPS).max(0);
    let i1 = (max_x - half).div_euclid(SUBSTEPS).min(ws - 1);
    let j0 = (min_y - half + SUBSTEPS - 1).div_euclid(SUBSTEPS).max(0);
    let j1 = (max_y - half).div_euclid(SUBSTEPS).min(hs - 1);
    if i0 > i1 || j0 > j1 {
        return;
    }
    // Edge k is opposite vertex k; its value at a point is that vertex's
    // barycentric weight times the area.
    let edges = [(p[1], p[2]), (p[2], p[0]), (p[0], p[1])];
    let value = |(a, b): ([i64; 2], [i64; 2]), q: [i64; 2]| {
        (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])
    };
    let step_x = edges.map(|(a, b)| -(b[1] - a[1]) * SUBSTEPS);
    let limit = edges.map(|(a, b)| if owns(a, b) { 0 } else { 1 });
    let inv_area = (1.0 / area as f64) as f32;
    let z = v.map(|v| v.depth);
    let lit = tri.lit;
    let level = tri.level;
    let cutoff = tri.cutoff;

    for j in j0..=j1 {
        let qy = j * SUBSTEPS + half;
        let q = [i0 * SUBSTEPS + half, qy];
        let mut e = [value(edges[0], q), value(edges[1], q), value(edges[2], q)];
        let row = j as usize * samples.width;
        for i in i0..=i1 {
            if e[0] >= limit[0] && e[1] >= limit[1] && e[2] >= limit[2] {
                let l = [
                    e[0] as f32 * inv_area,
                    e[1] as f32 * inv_area,
                    e[2] as f32 * inv_area,
                ];
                let index = row + i as usize;
                let depth = l[0] * z[0] + l[1] * z[1] + l[2] * z[2];
                if depth < samples.depth[index] {
                    let lerp2 = |f: fn(&Vertex) -> [f32; 2]| {
                        let (a, b, c) = (f(v[0]), f(v[1]), f(v[2]));
                        [
                            l[0] * a[0] + l[1] * b[0] + l[2] * c[0],
                            l[0] * a[1] + l[1] * b[1] + l[2] * c[1],
                        ]
                    };
                    let uv = lerp2(|v| v.uv);
                    let texel = texture.sample(level, uv);
                    let mut color = [0.0f32; 4];
                    for (k, c) in color.iter_mut().enumerate() {
                        let vc = l[0] * v[0].color[k] + l[1] * v[1].color[k] + l[2] * v[2].color[k];
                        *c = texel[k] * vc;
                    }
                    if cutoff < 0.0 || color[3] >= cutoff {
                        let factor = if lit {
                            let n = [0, 1, 2].map(|k| {
                                l[0] * v[0].normal[k]
                                    + l[1] * v[1].normal[k]
                                    + l[2] * v[2].normal[k]
                            });
                            shade.factor(n)
                        } else {
                            1.0
                        };
                        samples.color[index] = [
                            to_byte(color[0] * factor),
                            to_byte(color[1] * factor),
                            to_byte(color[2] * factor),
                            255,
                        ];
                        samples.depth[index] = depth;
                    }
                }
            }
            for k in 0..3 {
                e[k] += step_x[k];
            }
        }
    }
}
