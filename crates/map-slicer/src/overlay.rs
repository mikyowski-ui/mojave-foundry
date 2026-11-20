//! Ground overlays: flat meshes flattened onto the floor, rocks and cliffs
//! in relief, both baked into the floor tiles by the terrain stage.
//!
//! Each class renders as one scene, so overlapping pieces resolve through
//! the depth buffer rather than by list order. Flat meshes keep a sliver of
//! their height (`SlicerConfig::flatten`): the transform stays invertible
//! for the renderer, and where two flat pieces overlap the higher one still
//! wins, as it did standing. Relief keeps its full height, so a cliff reads
//! as a cliff, but it is floor art: it never occludes a character standing
//! behind it. That is an accepted limit of this pass.

use std::collections::BTreeSet;

use foundry_render::{Instance, Projection, RenderError, Renderer, Scene, TextureSource};
use mir::GroundSampler;
use nif::StaticMesh;

use crate::BakedOverlay;
use crate::hex::{self, Plane};
use crate::render::{pixel_rect, render_pieces, screen_bounds};

/// One mesh of a ground scene: its transform is already relative to the
/// terrain under its origin (and flattened, for flat meshes).
pub(crate) struct GroundPiece<'a> {
    pub mesh: &'a StaticMesh,
    pub transform: [[f32; 4]; 3],
}

/// Renders every piece into overlays: one per renderer window that holds
/// an opaque pixel, cropped to its content, clipped to the map's ground.
pub(crate) fn bake(
    renderer: &mut dyn Renderer,
    projection: &Projection,
    anchor: [f32; 3],
    pieces: &[GroundPiece<'_>],
    textures: &dyn TextureSource,
    side: u32,
) -> Result<Vec<BakedOverlay>, RenderError> {
    let bounds: Vec<Option<[i64; 4]>> = pieces
        .iter()
        .map(|p| {
            screen_bounds(projection, anchor, p.mesh, &p.transform).and_then(|b| pixel_rect(b, 2))
        })
        .collect();
    let map = hex::map_screen_bounds();
    let mut union = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for b in bounds.iter().flatten() {
        union = [
            union[0].min(b[0]),
            union[1].min(b[1]),
            union[2].max(b[2]),
            union[3].max(b[3]),
        ];
    }
    let rect = [
        union[0].max(map[0]),
        union[1].max(map[1]),
        union[2].min(map[2]),
        union[3].min(map[3]),
    ];
    let mut out = Vec::new();
    if rect[0] >= rect[2] || rect[1] >= rect[3] {
        return Ok(out);
    }
    let step = i64::from(side.max(1));
    let mut y = rect[1];
    while y < rect[3] {
        let h = (rect[3] - y).min(step);
        let mut x = rect[0];
        while x < rect[2] {
            let w = (rect[2] - x).min(step);
            let tile = [x, y, x + w, y + h];
            let instances: Vec<Instance<'_>> = pieces
                .iter()
                .zip(&bounds)
                .filter(|(_, b)| {
                    b.is_some_and(|b| {
                        b[0] < tile[2] && b[2] > tile[0] && b[1] < tile[3] && b[3] > tile[1]
                    })
                })
                .map(|(p, _)| Instance::new(p.mesh, p.transform, textures))
                .collect();
            if !instances.is_empty() {
                let scene = Scene { anchor, instances };
                render_pieces(renderer, &scene, tile, side, &mut |piece| {
                    if let Some(overlay) = crop(&piece) {
                        out.push(overlay);
                    }
                    Ok(())
                })?;
            }
            x += w;
        }
        y += h;
    }
    Ok(out)
}

fn crop(piece: &foundry_render::RenderOutput) -> Option<BakedOverlay> {
    let (w, h) = (piece.width as usize, piece.height as usize);
    let alpha = |x: usize, y: usize| piece.rgba[(y * w + x) * 4 + 3] != 0;
    let top = (0..h).find(|&y| (0..w).any(|x| alpha(x, y)))?;
    let bottom = (0..h).rev().find(|&y| (0..w).any(|x| alpha(x, y)))?;
    let left = (0..w).find(|&x| (top..=bottom).any(|y| alpha(x, y)))?;
    let right = (0..w)
        .rev()
        .find(|&x| (top..=bottom).any(|y| alpha(x, y)))?;
    let (cw, ch) = (right - left + 1, bottom - top + 1);
    let mut rgba = Vec::with_capacity(cw * ch * 4);
    for y in top..=bottom {
        rgba.extend_from_slice(&piece.rgba[(y * w + left) * 4..(y * w + right + 1) * 4]);
    }
    Some(BakedOverlay {
        origin: [piece.origin[0] + left as i32, piece.origin[1] + top as i32],
        width: cw as u32,
        height: ch as u32,
        rgba,
    })
}

/// Hexes whose centre lies under a triangle of the mesh that stands more
/// than `step` above the terrain there. `matrix` places the mesh in source
/// space; where no terrain was loaded, `fallback_ground` is used.
pub(crate) fn blocked_under(
    plane: &Plane,
    sampler: &GroundSampler,
    mesh: &StaticMesh,
    matrix: &[[f32; 4]; 3],
    fallback_ground: f32,
    step: f32,
    out: &mut BTreeSet<i32>,
) {
    for primitive in mesh.primitives.iter().filter(|p| p.is_well_formed()) {
        let placed: Vec<[f64; 3]> = primitive
            .positions
            .iter()
            .map(|p| {
                [0, 1, 2].map(|r| {
                    f64::from(
                        matrix[r][0] * p[0]
                            + matrix[r][1] * p[1]
                            + matrix[r][2] * p[2]
                            + matrix[r][3],
                    )
                })
            })
            .collect();
        for tri in primitive.indices.as_chunks::<3>().0 {
            let v = tri.map(|i| placed[i as usize]);
            if v.iter().flatten().any(|c| !c.is_finite()) {
                continue;
            }
            let st = v.map(|p| plane.tile_of_source([p[0], p[1]]));
            let area = (st[1][0] - st[0][0]) * (st[2][1] - st[0][1])
                - (st[1][1] - st[0][1]) * (st[2][0] - st[0][0]);
            if area.abs() < 1.0e-9 {
                continue;
            }
            for h in plane.hexes_in(&st) {
                if out.contains(&h) {
                    continue;
                }
                let Some(c) = plane.hex_tile(h) else {
                    continue;
                };
                // Barycentric height at the hex centre.
                let l1 = ((c[0] - st[0][0]) * (st[2][1] - st[0][1])
                    - (c[1] - st[0][1]) * (st[2][0] - st[0][0]))
                    / area;
                let l2 = ((st[1][0] - st[0][0]) * (c[1] - st[0][1])
                    - (st[1][1] - st[0][1]) * (c[0] - st[0][0]))
                    / area;
                let z = v[0][2] + l1 * (v[1][2] - v[0][2]) + l2 * (v[2][2] - v[0][2]);
                let src = plane.source_of_tile(c);
                let ground = sampler
                    .height(src[0] as f32, src[1] as f32)
                    .unwrap_or(fallback_ground);
                if z - f64::from(ground) > f64::from(step) {
                    out.insert(h);
                }
            }
        }
    }
}
