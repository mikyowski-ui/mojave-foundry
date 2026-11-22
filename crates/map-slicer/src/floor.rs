//! Walking surfaces of a sliced object.
//!
//! A porch floor is part of the building's mesh, so by default its planks
//! land in the wall strips of the hexes they lie over. The engine cuts wall
//! and scenery art in front of the player away inside a see-through circle,
//! and then whoever stands on the porch stands in a patch of bare terrain.
//!
//! So floors leave the strips. The mesh's triangles that face up and stand a
//! step or so above the terrain are rendered on their own, which also finds
//! the planks the nearest-surface strip render never sees under a porch
//! roof. Their pixels over hexes near one that is open
//! ([`crate::SlicerConfig::open_hexes`]) become floor pieces
//! ([`crate::Role::FloorPiece`]), and the same pixels are cleared from the
//! strip canvas where the floor was the nearest surface. Posts, railings
//! and the roof stay in the strips and are still cut by the circle.
//!
//! Floor pieces are drawn in the engine's flat pass, under every upright
//! object, so their order among themselves protects nothing and they merge
//! along hex rows freely.

use std::collections::{BTreeMap, BTreeSet};

use foundry_render::{Projection, Renderer, TextureSource};
use nif::{Primitive, StaticMesh};
use quantize::{Dither, PaletteMap};

use crate::SlicerConfig;
use crate::render::{self, Canvas};
use crate::strips::{self, CutOptions, Strip};

/// Two depths (metres along the view) of one surface.
const SAME_SURFACE: f32 = 0.02;
/// Floor pieces along a row's edge merge up to this many hexes.
const EDGE_RUN: usize = 8;

/// The triangles of `mesh` that are floors once placed by `matrix` (a
/// rotation and scale, no translation): facing up at least `min_up`, every
/// corner within `band` above the terrain, where `lift` is the height of the
/// mesh origin above the terrain. `None` when no triangle qualifies.
pub(crate) fn floor_mesh(
    mesh: &StaticMesh,
    matrix: &[[f32; 4]; 3],
    lift: f32,
    band: [f32; 2],
    min_up: f32,
) -> Option<StaticMesh> {
    let mut primitives = Vec::new();
    for primitive in mesh.primitives.iter().filter(|p| p.is_well_formed()) {
        let placed: Vec<[f32; 3]> = primitive
            .positions
            .iter()
            .map(|p| {
                [0, 1, 2].map(|r| {
                    matrix[r][0] * p[0] + matrix[r][1] * p[1] + matrix[r][2] * p[2] + matrix[r][3]
                })
            })
            .collect();
        let mut indices = Vec::new();
        for triangle in primitive.indices.as_chunks::<3>().0 {
            let corners = triangle.map(|i| placed[i as usize]);
            if is_floor(corners, primitive.material.two_sided, lift, band, min_up) {
                indices.extend_from_slice(triangle);
            }
        }
        if !indices.is_empty() {
            primitives.push(Primitive {
                indices,
                ..primitive.clone()
            });
        }
    }
    (!primitives.is_empty()).then(|| StaticMesh {
        primitives,
        collision: None,
        skipped_blocks: BTreeMap::new(),
    })
}

/// The upward component of a placed triangle's unit normal (its winding's
/// front face); `None` for a degenerate or non-finite triangle.
pub(crate) fn normal_up([a, b, c]: [[f32; 3]; 3]) -> Option<f32> {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    (length.is_finite() && length > 0.0).then(|| n[2] / length)
}

/// Whether a placed triangle (a rotation and scale applied, no translation)
/// is a floor: facing up at least `min_up` (either face, when two-sided),
/// every corner within `band` above the ground, where `lift` is the height
/// of the mesh origin above the ground.
pub(crate) fn is_floor(
    corners: [[f32; 3]; 3],
    two_sided: bool,
    lift: f32,
    band: [f32; 2],
    min_up: f32,
) -> bool {
    let Some(up) = normal_up(corners) else {
        return false;
    };
    let up = if two_sided { up.abs() } else { up };
    let in_band = corners.iter().all(|p| {
        let height = p[2] + lift;
        height >= band[0] && height <= band[1]
    });
    up >= min_up && in_band
}

/// The hexes within `reach` steps of a hex in `open`.
pub(crate) fn around(open: &BTreeSet<i32>, reach: i32) -> BTreeSet<i32> {
    let mut all = open.clone();
    let mut edge: Vec<i32> = open.iter().copied().collect();
    for _ in 0..reach.max(0) {
        let mut next = Vec::new();
        for hex in edge {
            for neighbour in crate::hex::neighbours(hex).into_iter().flatten() {
                if all.insert(neighbour) {
                    next.push(neighbour);
                }
            }
        }
        edge = next;
    }
    all
}

/// The floors of one placed mesh.
pub(crate) struct Floors {
    /// The floor render, canonical like the strip canvas; only the pixels
    /// that became floor pieces are opaque.
    pub canvas: Canvas,
    /// The pieces, sorted by hex.
    pub pieces: Vec<Strip>,
}

/// Renders the floors of `mesh` and cuts the part over `keep` into pieces.
/// `ground` is the mesh origin's ground point in map screen space, as for
/// [`strips::cut`]. `None` when the mesh has no floor there, or its render
/// fails: the strips then keep everything, as they did before.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pieces(
    renderer: &mut dyn Renderer,
    projection: &Projection,
    mesh: &StaticMesh,
    matrix: [[f32; 4]; 3],
    lift: f32,
    textures: &dyn TextureSource,
    map: &PaletteMap,
    ground: [i64; 2],
    keep: &BTreeSet<i32>,
    config: &SlicerConfig,
) -> Option<Floors> {
    let mesh = floor_mesh(mesh, &matrix, lift, config.floor_band, config.floor_min_up)?;
    let mut canvas = render::render_canonical(
        renderer,
        projection,
        &mesh,
        matrix,
        textures,
        config.render_tile,
        config.max_canvas_pixels,
    )
    .ok()??;
    let owners = strips::owners(&canvas, ground, projection);
    let mut any = false;
    for (index, owner) in owners.iter().enumerate() {
        if *owner >= 0 && keep.contains(owner) {
            any = true;
        } else {
            canvas.rgba[index * 4 + 3] = 0;
        }
    }
    if !any {
        return None;
    }
    let indexed =
        quantize::quantize(&canvas.rgba, canvas.width, canvas.height, map, Dither::None).ok()?;
    let cut = strips::cut(
        &canvas,
        &indexed,
        ground,
        projection,
        &CutOptions {
            min_pixels: config.min_strip_pixels,
            merge_interior: true,
            max_perimeter_run: EDGE_RUN,
            blocks: &|_| true,
        },
    )
    .ok()?;
    (!cut.strips.is_empty()).then_some(Floors {
        canvas,
        pieces: cut.strips,
    })
}

/// Clears from the strip canvas (and its palette indices) every pixel where
/// a floor piece's surface is the one the strip render shows.
pub(crate) fn clear_from(strip: &mut Canvas, indexed: &mut [u8], floors: &Canvas) {
    let (sw, sh) = (i64::from(strip.width), i64::from(strip.height));
    let fw = floors.width as usize;
    for index in 0..fw * floors.height as usize {
        if !floors.opaque(index) {
            continue;
        }
        let x = i64::from(floors.origin[0]) + (index % fw) as i64 - i64::from(strip.origin[0]);
        let y = i64::from(floors.origin[1]) + (index / fw) as i64 - i64::from(strip.origin[1]);
        if x < 0 || y < 0 || x >= sw || y >= sh {
            continue;
        }
        let at = (y * sw + x) as usize;
        if strip.opaque(at) && (strip.depth[at] - floors.depth[index]).abs() <= SAME_SURFACE {
            strip.rgba[at * 4 + 3] = 0;
            indexed[at] = 0;
        }
    }
}
