//! Headless oblique parallel sprite renderer with colour and depth output.
//!
//! Static meshes ([`nif::StaticMesh`], source space, game units) are placed
//! by a 3 × 4 transform, mapped to tile space and projected with the
//! Fallout 2 screen matrix read from `vault_formats::geometry::HexGrid`
//! (`docs/contracts.md` section 4). The output is an RGBA8 image with hard
//! alpha and a depth buffer in the contract's depth units (section 5).
//!
//! Two backends share one preparation step (transform, culling, normals,
//! mip choice) and one resolve step (box filter, hard alpha threshold,
//! contrast):
//!
//! - [`CpuRenderer`] is the reference: a fixed-point rasterizer with exact
//!   edge functions in absolute sub-sample coordinates and a top-left fill
//!   rule, so its output does not depend on the window it is cut from and
//!   is byte-identical on every machine.
//! - [`GpuRenderer`] runs the same shading on `wgpu`, headless, and reads
//!   the sub-samples back for the CPU resolve. It agrees with the CPU path
//!   within [`GPU_TOLERANCE`], not byte for byte.
//!
//! The shading model: diffuse texture (bilinear, repeat wrap, one mip level
//! per triangle chosen from its texel-to-sample ratio) times vertex colour,
//! times `ambient + intensity · max(0, n · l)` for [`Shading::Lit`]
//! instances. Back faces are culled unless the material is two-sided; a
//! visible back face is lit with its normal turned towards the viewer.
//! `Mask` materials test alpha against their cutoff, `Blend` materials test
//! at 0.5, `Opaque` ignores alpha; an instance may override the cutoff and
//! draw every face ([`Instance::alpha_cutoff`], [`Instance::two_sided`]). A missing texture renders flat mid-grey
//! and is listed in [`RenderOutput::missing_textures`].

mod calibration;
mod cpu;
mod error;
mod gpu;
mod prepare;
mod projection;
mod resolve;
mod texture;
mod tone;

pub use calibration::Calibration;
pub use cpu::CpuRenderer;
pub use error::RenderError;
pub use gpu::{GPU_TOLERANCE, GpuRenderer, GpuTolerance};
pub use prepare::{MAX_SAMPLES, MAX_WINDOW_SIDE};
pub use projection::Projection;
pub use texture::{MAX_TEXTURE_SIDE, NoTextures, Texture, TextureSource};
pub use tone::{ClassTones, Tone};

/// How an instance's colours are shaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shading {
    /// Lambert key light plus ambient fill.
    #[default]
    Lit,
    /// Texture times vertex colour as given: for pre-shaded geometry.
    Unlit,
}

/// One placed mesh.
#[derive(Clone, Copy)]
pub struct Instance<'a> {
    /// The mesh, in its own source-space coordinates.
    pub mesh: &'a nif::StaticMesh,
    /// Rows of the mesh-to-source affine transform, as
    /// `mir::Transform::matrix` gives it: `p' = M · [p, 1]`.
    pub transform: [[f32; 4]; 3],
    /// Texture path to RGBA8 image.
    pub textures: &'a dyn TextureSource,
    /// Lit or pre-shaded.
    pub shading: Shading,
    /// Draw both faces of every triangle, as if every material were
    /// two-sided: a sign whose letters are open at the back reads solid
    /// from behind. `false` (the default) keeps each material's own choice.
    pub two_sided: bool,
    /// The alpha test cutoff of every `Mask` and `Blend` material, instead
    /// of the material's own. A low cutoff keeps a thin alpha-tested wire
    /// or lattice texture once it is minified (its mip levels average the
    /// wires into partial alpha): the lattice renders as a solid surface of
    /// its colour instead of vanishing. `None` (the default) keeps the
    /// material's.
    pub alpha_cutoff: Option<f32>,
}

impl<'a> Instance<'a> {
    /// The identity transform.
    pub const IDENTITY: [[f32; 4]; 3] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];

    /// A lit instance.
    pub fn new(
        mesh: &'a nif::StaticMesh,
        transform: [[f32; 4]; 3],
        textures: &'a dyn TextureSource,
    ) -> Self {
        Instance {
            mesh,
            transform,
            textures,
            shading: Shading::Lit,
            two_sided: false,
            alpha_cutoff: None,
        }
    }

    /// The same instance with another shading.
    pub fn with_shading(mut self, shading: Shading) -> Self {
        self.shading = shading;
        self
    }

    /// The same instance with every face drawn ([`Instance::two_sided`]).
    pub fn with_two_sided(mut self, two_sided: bool) -> Self {
        self.two_sided = two_sided;
        self
    }

    /// The same instance with an alpha cutoff of its own
    /// ([`Instance::alpha_cutoff`]).
    pub fn with_alpha_cutoff(mut self, cutoff: Option<f32>) -> Self {
        self.alpha_cutoff = cutoff;
        self
    }
}

/// Everything drawn in one render.
#[derive(Clone, Default)]
pub struct Scene<'a> {
    /// The window anchor `(x_west, y_north, ground_z)` in source space; it
    /// projects to screen `(0, 0)`.
    pub anchor: [f32; 3],
    /// The instances, drawn in order (earlier wins a depth tie).
    pub instances: Vec<Instance<'a>>,
}

/// Which part of the screen to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// The scene's projected bounds, padded by one pixel on every side. An
    /// empty scene gives a 0 × 0 image.
    Fit,
    /// Exactly this rectangle of screen pixels, the projected anchor at
    /// `(0, 0)`. Pixel `(i, j)` covers screen `[x + i, x + i + 1) × [y + j,
    /// y + j + 1)`.
    Rect {
        /// Left edge.
        x: i32,
        /// Top edge.
        y: i32,
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
}

/// Which rasterizer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The GPU when an adapter exists, else the CPU.
    #[default]
    Auto,
    /// The reference software rasterizer.
    Cpu,
    /// `wgpu`, headless.
    Gpu,
}

/// A rendered image (`docs/contracts.md` section 5 buffers).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOutput {
    /// Screen position of pixel `(0, 0)`, relative to the projected anchor.
    pub origin: [i32; 2],
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA8, straight alpha; alpha is 0 or 255.
    pub rgba: Vec<u8>,
    /// Depth of the nearest covered sub-sample, `f32::INFINITY` where the
    /// pixel is transparent.
    pub depth: Vec<f32>,
    /// The backend that produced the image: [`Backend::Cpu`] or
    /// [`Backend::Gpu`].
    pub backend: Backend,
    /// Distinct texture paths that were missing (or unusable), sorted. Each
    /// rendered flat mid-grey.
    pub missing_textures: Vec<String>,
    /// Triangles rasterized (front faces and visible two-sided back faces).
    pub triangles: u32,
    /// Triangles dropped as unusable: non-finite or out-of-range vertices,
    /// malformed primitives, singular transforms.
    pub skipped_triangles: u32,
}

impl RenderOutput {
    /// Number of opaque pixels.
    pub fn opaque_pixels(&self) -> usize {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] != 0)
            .count()
    }

    /// The pixel at `(x, y)`, if inside.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba.get(i..i + 4).map(|p| [p[0], p[1], p[2], p[3]])
    }

    /// The pixel covering screen point `(sx, sy)`, if inside.
    pub fn pixel_at_screen(&self, sx: f32, sy: f32) -> Option<[u8; 4]> {
        let (x, y) = self.index_of_screen(sx, sy)?;
        self.pixel(x, y)
    }

    /// The depth of the pixel covering screen point `(sx, sy)`, if inside.
    pub fn depth_at_screen(&self, sx: f32, sy: f32) -> Option<f32> {
        let (x, y) = self.index_of_screen(sx, sy)?;
        self.depth
            .get(y as usize * self.width as usize + x as usize)
            .copied()
    }

    fn index_of_screen(&self, sx: f32, sy: f32) -> Option<(u32, u32)> {
        let x = (sx - self.origin[0] as f32).floor();
        let y = (sy - self.origin[1] as f32).floor();
        (x >= 0.0 && y >= 0.0 && x < self.width as f32 && y < self.height as f32)
            .then_some((x as u32, y as u32))
    }
}

/// A sprite renderer.
pub trait Renderer {
    /// Renders `scene` into `window`.
    fn render(&mut self, scene: &Scene, window: Window) -> Result<RenderOutput, RenderError>;

    /// The backend this renderer runs: [`Backend::Cpu`] or [`Backend::Gpu`].
    fn backend(&self) -> Backend;
}

/// A renderer for `backend`. `Auto` tries the GPU first and falls back to
/// the CPU when no adapter is available; `Gpu` fails instead.
pub fn renderer(
    backend: Backend,
    calibration: &Calibration,
) -> Result<Box<dyn Renderer>, RenderError> {
    match backend {
        Backend::Cpu => Ok(Box::new(CpuRenderer::new(calibration)?)),
        Backend::Gpu => Ok(Box::new(GpuRenderer::new(calibration)?)),
        Backend::Auto => {
            calibration.validate()?;
            match GpuRenderer::new(calibration) {
                Ok(gpu) => Ok(Box::new(gpu)),
                Err(_) => Ok(Box::new(CpuRenderer::new(calibration)?)),
            }
        }
    }
}
