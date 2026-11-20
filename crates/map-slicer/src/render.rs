//! Rendering placed meshes into map screen space, in pieces when large.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;

use foundry_render::{
    Instance, MAX_WINDOW_SIDE, Projection, RenderError, Renderer, Scene, Texture, TextureSource,
    Window,
};
use mir::Mir;
use nif::StaticMesh;

/// Where meshes and textures come from: a MIR directory, or anything else
/// a caller (or a test) provides.
pub trait Assets {
    /// The mesh with this content hash.
    fn mesh(&self, hash: &str) -> Result<StaticMesh, String>;
    /// The textures meshes name.
    fn textures(&self) -> &dyn TextureSource;
}

/// [`Assets`] over a MIR directory, decoding each texture once while the
/// cache stays under its byte limit.
pub struct MirAssets<'a> {
    mir: &'a Mir,
    textures: MirTextures<'a>,
}

impl<'a> MirAssets<'a> {
    /// Assets of `mir`, keeping at most `cache_bytes` of decoded textures.
    pub fn new(mir: &'a Mir, cache_bytes: usize) -> Self {
        MirAssets {
            mir,
            textures: MirTextures {
                mir,
                limit: cache_bytes,
                cache: RefCell::new((BTreeMap::new(), 0)),
            },
        }
    }
}

impl Assets for MirAssets<'_> {
    fn mesh(&self, hash: &str) -> Result<StaticMesh, String> {
        self.mir.mesh(hash).map_err(|e| e.to_string())
    }

    fn textures(&self) -> &dyn TextureSource {
        &self.textures
    }
}

type TextureCache = (BTreeMap<String, Option<Texture>>, usize);

struct MirTextures<'a> {
    mir: &'a Mir,
    limit: usize,
    cache: RefCell<TextureCache>,
}

impl TextureSource for MirTextures<'_> {
    fn texture(&self, path: &str) -> Option<Cow<'_, Texture>> {
        let mut guard = self.cache.borrow_mut();
        let (cache, bytes) = &mut *guard;
        if let Some(slot) = cache.get(path) {
            return slot.clone().map(Cow::Owned);
        }
        let texture = (|| {
            let hash = self.mir.texture_hash(path)?;
            let image = self.mir.texture(hash).ok()?;
            Some(Texture {
                width: image.width,
                height: image.height,
                rgba: image.rgba,
            })
        })();
        let size = texture.as_ref().map_or(0, |t| t.rgba.len());
        if *bytes + size > self.limit {
            cache.clear();
            *bytes = 0;
        }
        *bytes += size;
        cache.insert(path.to_owned(), texture.clone());
        texture.map(Cow::Owned)
    }
}

/// A rendered image in map screen space (contracts section 5 buffers).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Canvas {
    /// Screen position of pixel (0, 0), relative to the scene's projected
    /// anchor.
    pub origin: [i32; 2],
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub depth: Vec<f32>,
}

impl Canvas {
    pub fn opaque(&self, index: usize) -> bool {
        self.rgba[index * 4 + 3] != 0
    }
}

/// Screen bounds `[x0, y0, x1, y1]` of a mesh placed by `transform` in a
/// scene anchored at `anchor`, over well-formed primitives; `None` without
/// a finite vertex.
pub(crate) fn screen_bounds(
    projection: &Projection,
    anchor: [f32; 3],
    mesh: &StaticMesh,
    transform: &[[f32; 4]; 3],
) -> Option<[f64; 4]> {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for primitive in mesh.primitives.iter().filter(|p| p.is_well_formed()) {
        for p in &primitive.positions {
            let src = [0, 1, 2].map(|r| {
                transform[r][0] * p[0]
                    + transform[r][1] * p[1]
                    + transform[r][2] * p[2]
                    + transform[r][3]
            });
            let s = projection.screen(projection.tile_from_source(anchor, src));
            if s.iter().all(|v| v.is_finite()) {
                b = [
                    b[0].min(f64::from(s[0])),
                    b[1].min(f64::from(s[1])),
                    b[2].max(f64::from(s[0])),
                    b[3].max(f64::from(s[1])),
                ];
            }
        }
    }
    (b[0] <= b[2] && b[1] <= b[3]).then_some(b)
}

/// Integer pixel rectangle `[x0, y0, x1, y1)` covering float bounds with a
/// margin.
pub(crate) fn pixel_rect(b: [f64; 4], margin: i64) -> Option<[i64; 4]> {
    let limit = 1.0e9;
    if b.iter().any(|v| !v.is_finite() || v.abs() > limit) {
        return None;
    }
    Some([
        b[0].floor() as i64 - margin,
        b[1].floor() as i64 - margin,
        b[2].ceil() as i64 + margin,
        b[3].ceil() as i64 + margin,
    ])
}

/// Renders `scene` over the rectangle `rect` (`[x0, y0, x1, y1)`), cutting
/// it into windows of at most `side` pixels a side, and calling `piece` with
/// each window's output. A window the renderer refuses as too large (a
/// higher supersample than the side was chosen for) is split in four.
pub(crate) fn render_pieces(
    renderer: &mut dyn Renderer,
    scene: &Scene,
    rect: [i64; 4],
    side: u32,
    piece: &mut dyn FnMut(foundry_render::RenderOutput) -> Result<(), RenderError>,
) -> Result<(), RenderError> {
    let side = i64::from(side.clamp(1, MAX_WINDOW_SIDE));
    let mut y = rect[1];
    while y < rect[3] {
        let h = (rect[3] - y).min(side);
        let mut x = rect[0];
        while x < rect[2] {
            let w = (rect[2] - x).min(side);
            render_window(renderer, scene, [x, y, w, h], piece)?;
            x += w;
        }
        y += h;
    }
    Ok(())
}

fn render_window(
    renderer: &mut dyn Renderer,
    scene: &Scene,
    [x, y, w, h]: [i64; 4],
    piece: &mut dyn FnMut(foundry_render::RenderOutput) -> Result<(), RenderError>,
) -> Result<(), RenderError> {
    let window = Window::Rect {
        x: x as i32,
        y: y as i32,
        width: w as u32,
        height: h as u32,
    };
    match renderer.render(scene, window) {
        Ok(out) => piece(out),
        Err(RenderError::WindowTooLarge { .. }) if w > 1 || h > 1 => {
            let (w0, h0) = ((w + 1) / 2, (h + 1) / 2);
            for (dx, dy, pw, ph) in [
                (0, 0, w0, h0),
                (w0, 0, w - w0, h0),
                (0, h0, w0, h - h0),
                (w0, h0, w - w0, h - h0),
            ] {
                if pw > 0 && ph > 0 {
                    render_window(renderer, scene, [x + dx, y + dy, pw, ph], piece)?;
                }
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Renders one mesh with `transform` in a scene anchored at the origin,
/// so the mesh origin projects to screen `(0, 0)`, into one canvas.
pub(crate) fn render_canonical(
    renderer: &mut dyn Renderer,
    projection: &Projection,
    mesh: &StaticMesh,
    transform: [[f32; 4]; 3],
    textures: &dyn TextureSource,
    side: u32,
    max_pixels: u64,
) -> Result<Option<Canvas>, CanvasError> {
    let anchor = [0.0; 3];
    let Some(bounds) = screen_bounds(projection, anchor, mesh, &transform) else {
        return Ok(None);
    };
    let Some(rect) = pixel_rect(bounds, 2) else {
        return Err(CanvasError::TooLarge);
    };
    let (w, h) = ((rect[2] - rect[0]) as u64, (rect[3] - rect[1]) as u64);
    if w.saturating_mul(h) > max_pixels || w > u64::from(u32::MAX) || h > u64::from(u32::MAX) {
        return Err(CanvasError::TooLarge);
    }
    let (w, h) = (w as usize, h as usize);
    let mut canvas = Canvas {
        origin: [rect[0] as i32, rect[1] as i32],
        width: w as u32,
        height: h as u32,
        rgba: vec![0; w * h * 4],
        depth: vec![f32::INFINITY; w * h],
    };
    let scene = Scene {
        anchor,
        instances: vec![Instance::new(mesh, transform, textures)],
    };
    render_pieces(renderer, &scene, rect, side, &mut |out| {
        let ox = (i64::from(out.origin[0]) - rect[0]) as usize;
        let oy = (i64::from(out.origin[1]) - rect[1]) as usize;
        let ow = out.width as usize;
        for row in 0..out.height as usize {
            let src = row * ow;
            let dst = (oy + row) * w + ox;
            canvas.rgba[dst * 4..(dst + ow) * 4]
                .copy_from_slice(&out.rgba[src * 4..(src + ow) * 4]);
            canvas.depth[dst..dst + ow].copy_from_slice(&out.depth[src..src + ow]);
        }
        Ok(())
    })
    .map_err(CanvasError::Render)?;
    Ok(Some(canvas))
}

#[derive(Debug)]
pub(crate) enum CanvasError {
    TooLarge,
    Render(RenderError),
}
