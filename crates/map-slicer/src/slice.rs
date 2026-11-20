//! The run: classify, render, cut, deduplicate, account.

use std::collections::{BTreeMap, BTreeSet};

use emit::PlacedFrame;
use foundry_render::{Projection, Renderer};
use mir::{GroundSampler, MapWindow, Mir, Reference, Transform};
use nif::StaticMesh;
use quantize::{Dither, PaletteMap};
use sha2::{Digest, Sha256};

use crate::classify::{Footprint, MeshFacts};
use crate::floor;
use crate::hex::{self, Plane};
use crate::overlay::{self, GroundPiece};
use crate::render::{self, Assets, CanvasError, MirAssets};
use crate::report::{BudgetReport, Contributor, Spread};
use crate::strips;
use crate::{
    ArtCategory, ArtImage, Budget, Class, ClassKind, Placement, Role, SkipReason, Skipped,
    SliceError, Sliced, SlicerConfig, classify,
};

/// Slices the static references of one map window: see the crate
/// documentation. Meshes and textures come from `mir`.
#[allow(clippy::too_many_arguments)]
pub fn slice_statics(
    mir: &Mir,
    references: &[Reference],
    window: &MapWindow,
    sampler: &GroundSampler,
    projection: &Projection,
    renderer: &mut dyn Renderer,
    map: &PaletteMap,
    budget: &Budget,
    config: &SlicerConfig,
) -> Result<Sliced, SliceError> {
    let assets = MirAssets::new(mir, config.texture_cache_bytes);
    slice_with(
        &assets, references, window, sampler, projection, renderer, map, budget, config,
    )
}

/// Counts what [`slice_statics`] would need without rendering anything:
/// references are classified and sliced objects counted by the hexes their
/// footprint covers (perimeter hexes plus one per interior run when runs are
/// merged). Meshes are read for their bounds and triangle area.
pub fn estimate(
    mir: &Mir,
    references: &[Reference],
    window: &MapWindow,
    sampler: &GroundSampler,
    projection: &Projection,
    budget: &Budget,
    config: &SlicerConfig,
) -> Result<BudgetReport, SliceError> {
    let assets = MirAssets::new(mir, 0);
    estimate_with(
        &assets, references, window, sampler, projection, budget, config,
    )
}

/// The cache key of a render: everything but the translation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RenderKey {
    pub(crate) mesh: String,
    rotation: [i64; 3],
    scale: i64,
}

impl RenderKey {
    pub(crate) fn of(reference: &Reference, config: &SlicerConfig) -> Option<Self> {
        let q = |v: f32, step: f32| (f64::from(v) / f64::from(step)).round() as i64;
        Some(RenderKey {
            mesh: reference.mesh.clone()?,
            rotation: reference
                .transform
                .rotation
                .map(|a| q(a, config.rotation_quantum)),
            scale: q(reference.transform.scale, config.scale_quantum),
        })
    }

    /// The canonical transform: no translation.
    pub(crate) fn matrix(&self, config: &SlicerConfig) -> [[f32; 4]; 3] {
        let d = |v: i64, step: f32| (v as f64 * f64::from(step)) as f32;
        Transform {
            position: [0.0; 3],
            rotation: self.rotation.map(|a| d(a, config.rotation_quantum)),
            scale: d(self.scale, config.scale_quantum),
        }
        .matrix()
    }
}

type MeshSlot = Result<(StaticMesh, MeshFacts), String>;

/// References sorted and classified, with their meshes.
struct Prepared<'r> {
    refs: Vec<&'r Reference>,
    classes: Vec<Class>,
    meshes: BTreeMap<String, MeshSlot>,
}

fn prepare<'r>(
    assets: &dyn Assets,
    references: &'r [Reference],
    tile_units: f32,
    config: &SlicerConfig,
) -> Prepared<'r> {
    let mut refs: Vec<&Reference> = references.iter().collect();
    refs.sort_by_cached_key(|r| r.form.to_string());
    refs.dedup_by(|a, b| a.form == b.form);
    let enabled: BTreeMap<String, bool> = refs
        .iter()
        .map(|r| {
            (
                r.form.to_string(),
                !(r.flags.deleted || r.flags.initially_disabled),
            )
        })
        .collect();
    let mut meshes: BTreeMap<String, MeshSlot> = BTreeMap::new();
    let mut classes = Vec::with_capacity(refs.len());
    for r in &refs {
        let mut class = classify(r, None, tile_units, config);
        if class == Class::Skip(SkipReason::NoMesh)
            && let (Some(_), Some(hash)) = (&r.model, &r.mesh)
        {
            let slot = meshes.entry(hash.clone()).or_insert_with(|| {
                assets.mesh(hash).map(|mesh| {
                    let facts = MeshFacts::of(&mesh);
                    (mesh, facts)
                })
            });
            class = match slot {
                Ok((_, facts)) => classify(r, Some(facts), tile_units, config),
                Err(e) => Class::Skip(SkipReason::Unreadable(e.clone())),
            };
        }
        if config.follow_enable_parents
            && !matches!(class, Class::Skip(_))
            && let Some(parent) = &r.enable_parent
        {
            let parent_on = enabled
                .get(&parent.reference.to_string())
                .copied()
                .unwrap_or(true);
            if parent_on == parent.opposite {
                class = Class::Skip(SkipReason::DisabledByParent);
            }
        }
        classes.push(class);
    }
    Prepared {
        refs,
        classes,
        meshes,
    }
}

/// Where a reference stands in map screen space: its ground point rounded
/// to a pixel, the rounded offset from there to its origin (its height),
/// and the terrain height under it.
pub(crate) fn standing(
    projection: &Projection,
    window: &MapWindow,
    sampler: &GroundSampler,
    reference: &Reference,
) -> Option<([i64; 2], [i32; 2], f32)> {
    let [x, y, z] = reference.transform.position;
    let ground_z = sampler.height(x, y).unwrap_or(z);
    let anchor = [window.x_west, window.y_north, ground_z];
    let g = projection.screen(projection.tile_from_source(anchor, [x, y, ground_z]));
    let p = projection.screen(projection.tile_from_source(anchor, [x, y, z]));
    let ok = |v: f32| v.is_finite() && v.abs() < 1.0e8;
    if !(g.iter().all(|v| ok(*v)) && p.iter().all(|v| ok(*v))) {
        return None;
    }
    let ground = [g[0].round() as i64, g[1].round() as i64];
    let offset = [(p[0] - g[0]).round() as i32, (p[1] - g[1]).round() as i32];
    Some((ground, offset, ground_z))
}

/// Art deduplicated by content, with its accounting.
#[derive(Default)]
struct ArtStore {
    art: Vec<ArtImage>,
    index: BTreeMap<[u8; 32], usize>,
    /// (category, model, class) -> (references, entries).
    contributions: BTreeMap<(ArtCategory, String, ClassKind), (usize, usize)>,
    single_sizes: (Vec<u32>, Vec<u32>),
}

impl ArtStore {
    /// The index of the art, adding it when new; `true` when it was new.
    fn add(
        &mut self,
        category: ArtCategory,
        frame: &PlacedFrame,
        model: &str,
        class: ClassKind,
    ) -> (usize, bool) {
        let mut hasher = Sha256::new();
        hasher.update([category.index() as u8]);
        hasher.update(frame.frame.width.to_le_bytes());
        hasher.update(frame.frame.height.to_le_bytes());
        hasher.update(frame.shift[0].to_le_bytes());
        hasher.update(frame.shift[1].to_le_bytes());
        hasher.update(&frame.frame.pixels);
        let digest: [u8; 32] = hasher.finalize().into();
        if let Some(&i) = self.index.get(&digest) {
            return (i, false);
        }
        let i = self.art.len();
        self.art.push(ArtImage {
            category,
            width: u32::from(frame.frame.width),
            height: u32::from(frame.frame.height),
            pixels: frame.frame.pixels.clone(),
            shift: frame.shift,
            source: model.to_owned(),
        });
        self.index.insert(digest, i);
        self.contributions
            .entry((category, model.to_owned(), class))
            .or_default()
            .1 += 1;
        (i, true)
    }

    fn note_reference(&mut self, category: ArtCategory, model: &str, class: ClassKind) {
        self.contributions
            .entry((category, model.to_owned(), class))
            .or_default()
            .0 += 1;
    }

    fn contributors(&self) -> Vec<Contributor> {
        self.contributions
            .iter()
            .map(
                |((category, model, class), (references, entries))| Contributor {
                    category: *category,
                    model: model.clone(),
                    class: *class,
                    references: *references,
                    entries: *entries,
                },
            )
            .collect()
    }
}

/// A skip found after classification moves its reference out of its class.
fn demote(report: &mut BudgetReport, kind: ClassKind) {
    report.class_mut(kind).references -= 1;
    report.class_mut(ClassKind::Skipped).references += 1;
}

/// Distance from a point to a convex polygon (0 inside).
fn distance_to_polygon(polygon: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = polygon.len();
    if n == 0 {
        return f64::INFINITY;
    }
    if n >= 3 {
        let sign = if crate::hex::signed_area(polygon) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let inside = (0..n).all(|i| {
            let a = polygon[i];
            let b = polygon[(i + 1) % n];
            ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) * sign >= 0.0
        });
        if inside {
            return 0.0;
        }
    }
    (0..n)
        .map(|i| {
            let a = polygon[i];
            let b = polygon[(i + 1) % n];
            let ab = [b[0] - a[0], b[1] - a[1]];
            let len = ab[0] * ab[0] + ab[1] * ab[1];
            let t = if len > 0.0 {
                (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [a[0] + t * ab[0], a[1] + t * ab[1]];
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

/// A strip blocks when its hex centre is within this many floor squares of
/// the object's footprint: about half a hex, so a thin fence still blocks
/// the hexes it crosses while eaves over open ground do not.
const BLOCK_REACH: f64 = 0.3;

/// [`slice_statics`] over any [`Assets`].
#[allow(clippy::too_many_arguments)]
pub fn slice_with(
    assets: &dyn Assets,
    references: &[Reference],
    window: &MapWindow,
    sampler: &GroundSampler,
    projection: &Projection,
    renderer: &mut dyn Renderer,
    map: &PaletteMap,
    budget: &Budget,
    config: &SlicerConfig,
) -> Result<Sliced, SliceError> {
    config.validate()?;
    let plane =
        Plane::new(projection, window.x_west, window.y_north).ok_or(SliceError::Projection)?;
    let tile_units = projection.tile_units();
    let prepared = prepare(assets, references, tile_units, config);
    let mut report = BudgetReport::new(false, budget);
    let mut skipped = Vec::new();
    let mut placements = Vec::new();
    let mut store = ArtStore::default();
    let mut blocked = BTreeSet::new();
    let skip = |skipped: &mut Vec<Skipped>, r: &Reference, reason: SkipReason| {
        skipped.push(Skipped {
            reference: r.form.to_string(),
            model: r.model.clone(),
            reason,
        });
    };

    // Group the sprite and sliced references by render.
    let mut groups: BTreeMap<RenderKey, Vec<usize>> = BTreeMap::new();
    let mut flat = Vec::new();
    let mut relief = Vec::new();
    for (i, (r, class)) in prepared.refs.iter().zip(&prepared.classes).enumerate() {
        report.class_mut(class.kind()).references += 1;
        match class {
            Class::Skip(reason) => skip(&mut skipped, r, reason.clone()),
            Class::GroundFlat => flat.push(i),
            Class::GroundRelief => relief.push(i),
            Class::Sprite(_) | Class::Sliced => match RenderKey::of(r, config) {
                Some(key) => groups.entry(key).or_default().push(i),
                None => skip(&mut skipped, r, SkipReason::NoMesh),
            },
        }
    }

    let textures = assets.textures();
    let near_open = floor::around(&config.open_hexes, config.floor_reach);
    let mut strip_runs = 0usize;
    for (key, members) in &groups {
        let Some(Ok((mesh, _))) = prepared.meshes.get(&key.mesh) else {
            continue;
        };
        let first_kind = prepared.classes[members[0]].kind();
        let fail_all =
            |reason: SkipReason, skipped: &mut Vec<Skipped>, report: &mut BudgetReport| {
                for &i in members {
                    skip(skipped, prepared.refs[i], reason.clone());
                    demote(report, prepared.classes[i].kind());
                }
            };
        let canvas = match render::render_canonical(
            renderer,
            projection,
            mesh,
            key.matrix(config),
            textures,
            config.render_tile,
            config.max_canvas_pixels,
        ) {
            Ok(Some(canvas)) => canvas,
            Ok(None) => {
                fail_all(SkipReason::NoGeometry, &mut skipped, &mut report);
                continue;
            }
            Err(CanvasError::TooLarge) => {
                fail_all(SkipReason::TooLarge, &mut skipped, &mut report);
                continue;
            }
            Err(CanvasError::Render(e)) => {
                fail_all(
                    SkipReason::Failed(format!("render: {e}")),
                    &mut skipped,
                    &mut report,
                );
                continue;
            }
        };
        if !(0..canvas.width as usize * canvas.height as usize).any(|i| canvas.opaque(i)) {
            fail_all(SkipReason::EmptyRender, &mut skipped, &mut report);
            continue;
        }
        let indexed = match quantize::quantize(
            &canvas.rgba,
            canvas.width,
            canvas.height,
            map,
            Dither::None,
        ) {
            Ok(indexed) => indexed,
            Err(e) => {
                fail_all(
                    SkipReason::Failed(format!("quantize: {e}")),
                    &mut skipped,
                    &mut report,
                );
                continue;
            }
        };
        report.class_mut(first_kind).renders += 1;
        let mut sprite_frame: Option<Result<PlacedFrame, String>> = None;

        for &i in members {
            let r = prepared.refs[i];
            let class = &prepared.classes[i];
            let model = r.model.clone().unwrap_or_default();
            let Some((ground, offset, ground_z)) = standing(projection, window, sampler, r) else {
                skip(&mut skipped, r, SkipReason::OutsideMap);
                demote(&mut report, class.kind());
                continue;
            };
            match class {
                Class::Sprite(role) => {
                    let Some(h) = hex::pick([ground[0] as f64, ground[1] as f64]) else {
                        skip(&mut skipped, r, SkipReason::OutsideMap);
                        demote(&mut report, class.kind());
                        continue;
                    };
                    let frame = sprite_frame.get_or_insert_with(|| {
                        emit::placed_frame(&indexed, canvas.width, canvas.height, canvas.origin)
                            .map_err(|e| e.to_string())
                    });
                    let frame = match frame {
                        Ok(frame) => frame,
                        Err(e) => {
                            skip(&mut skipped, r, SkipReason::Failed(format!("frame: {e}")));
                            demote(&mut report, class.kind());
                            continue;
                        }
                    };
                    let Some(centre) = hex::center(h) else {
                        continue;
                    };
                    let displacement = [
                        ground[0] + i64::from(offset[0]) - centre[0],
                        ground[1] + i64::from(offset[1]) - centre[1],
                    ];
                    let (Ok(dx), Ok(dy)) = (
                        i32::try_from(displacement[0]),
                        i32::try_from(displacement[1]),
                    ) else {
                        skip(&mut skipped, r, SkipReason::OutsideMap);
                        demote(&mut report, class.kind());
                        continue;
                    };
                    let category = role.category();
                    let (art, new) = store.add(category, frame, &model, ClassKind::Sprite);
                    if new {
                        report.class_mut(ClassKind::Sprite).entries += 1;
                    }
                    store.note_reference(category, &model, ClassKind::Sprite);
                    placements.push(Placement {
                        art,
                        hex: h,
                        displacement: [dx, dy],
                        role: *role,
                        reference: r.form.to_string(),
                        blocking: role.blocking(),
                        run: None,
                    });
                }
                Class::Sliced => {
                    let polygon: Vec<[f64; 2]> = prepared
                        .meshes
                        .get(&key.mesh)
                        .and_then(|m| m.as_ref().ok())
                        .and_then(|(_, facts)| facts.bounds)
                        .map(|b| Footprint::of(&r.transform, b).polygon)
                        .unwrap_or_default()
                        .iter()
                        .map(|p| plane.tile_of_source(*p))
                        .collect();
                    let blocks = |h: i32| {
                        !config.open_hexes.contains(&h)
                            && plane
                                .hex_tile(h)
                                .is_some_and(|c| distance_to_polygon(&polygon, c) <= BLOCK_REACH)
                    };
                    // Walking surfaces by an open hex leave the strips (see
                    // the `floor` module).
                    let has_open = config.open_hexes.iter().any(|&h| {
                        plane
                            .hex_tile(h)
                            .is_some_and(|c| distance_to_polygon(&polygon, c) <= BLOCK_REACH)
                    });
                    let floors = if has_open {
                        floor::pieces(
                            renderer,
                            projection,
                            mesh,
                            key.matrix(config),
                            r.transform.position[2] - ground_z,
                            textures,
                            map,
                            ground,
                            &near_open,
                            config,
                        )
                    } else {
                        None
                    };
                    let without_floors = floors.as_ref().map(|floors| {
                        let mut canvas = canvas.clone();
                        let mut indexed = indexed.clone();
                        floor::clear_from(&mut canvas, &mut indexed, &floors.canvas);
                        (canvas, indexed)
                    });
                    let (strip_canvas, strip_indexed) = match &without_floors {
                        Some((canvas, indexed)) => (canvas, indexed),
                        None => (&canvas, &indexed),
                    };
                    let floor_pieces = floors.map(|f| f.pieces).unwrap_or_default();
                    let options = strips::CutOptions {
                        min_pixels: config.min_strip_pixels,
                        merge_interior: config.merge_interior_runs,
                        max_perimeter_run: config.max_perimeter_run,
                        blocks: &blocks,
                    };
                    let cut = match strips::cut(
                        strip_canvas,
                        strip_indexed,
                        ground,
                        projection,
                        &options,
                    ) {
                        Ok(cut) => cut,
                        Err(e) => {
                            skip(&mut skipped, r, SkipReason::Failed(e));
                            demote(&mut report, class.kind());
                            continue;
                        }
                    };
                    report.strips.dropped_pixels += cut.dropped;
                    if cut.strips.is_empty() && floor_pieces.is_empty() {
                        skip(&mut skipped, r, SkipReason::OutsideMap);
                        demote(&mut report, class.kind());
                        continue;
                    }
                    report.strips.merged_tiny += cut.merged_tiny;
                    report.strips.folded += cut.folded;
                    blocked.extend(cut.run_hexes.iter().copied());
                    store.note_reference(ArtCategory::Walls, &model, ClassKind::Sliced);
                    for strip in &cut.strips {
                        let (art, new) =
                            store.add(ArtCategory::Walls, &strip.frame, &model, ClassKind::Sliced);
                        if new {
                            report.class_mut(ClassKind::Sliced).entries += 1;
                            if strip.run.is_none() {
                                store
                                    .single_sizes
                                    .0
                                    .push(u32::from(strip.frame.frame.width));
                                store
                                    .single_sizes
                                    .1
                                    .push(u32::from(strip.frame.frame.height));
                            }
                        }
                        if strip.run.is_some() {
                            strip_runs += 1;
                        }
                        let blocking = strip.run.is_some() || blocks(strip.hex);
                        placements.push(Placement {
                            art,
                            hex: strip.hex,
                            displacement: offset,
                            role: Role::WallStrip,
                            reference: r.form.to_string(),
                            blocking,
                            run: strip.run,
                        });
                        report.strips.placed += 1;
                    }
                    if !floor_pieces.is_empty() {
                        store.note_reference(ArtCategory::Scenery, &model, ClassKind::Sliced);
                    }
                    for piece in &floor_pieces {
                        let (art, _) = store.add(
                            ArtCategory::Scenery,
                            &piece.frame,
                            &model,
                            ClassKind::Sliced,
                        );
                        placements.push(Placement {
                            art,
                            hex: piece.hex,
                            displacement: offset,
                            role: Role::FloorPiece,
                            reference: r.form.to_string(),
                            blocking: false,
                            run: None,
                        });
                        report.strips.floor_pieces += 1;
                    }
                }
                _ => {}
            }
        }
    }
    report.strips.runs = strip_runs;

    // Ground overlays.
    let anchor = [window.x_west, window.y_north, 0.0];
    let mut overlays = Vec::new();
    for (members, flatten) in [(&flat, true), (&relief, false)] {
        let mut pieces = Vec::new();
        for &i in members {
            let r = prepared.refs[i];
            let Some(Some(Ok((mesh, _)))) = r.mesh.as_ref().map(|h| prepared.meshes.get(h)) else {
                continue;
            };
            let [x, y, z] = r.transform.position;
            let ground_z = sampler.height(x, y).unwrap_or(z);
            let mut transform = r.transform.matrix();
            transform[2][3] -= ground_z;
            if flatten {
                transform[2] = transform[2].map(|v| v * config.flatten);
            } else {
                overlay::blocked_under(
                    &plane,
                    sampler,
                    mesh,
                    &r.transform.matrix(),
                    ground_z,
                    config.step_height,
                    &mut blocked,
                );
            }
            pieces.push(GroundPiece { mesh, transform });
            let kind = if flatten {
                ClassKind::GroundFlat
            } else {
                ClassKind::GroundRelief
            };
            report.class_mut(kind).renders += 1;
        }
        overlays.extend(overlay::bake(
            renderer,
            projection,
            anchor,
            &pieces,
            textures,
            config.render_tile,
        )?);
    }

    // Accounting.
    for image in &store.art {
        report.category_mut(image.category).used += 1;
    }
    report.strips.distinct = report.category(ArtCategory::Walls).used;
    report.strips.width = Spread::of(&mut store.single_sizes.0);
    report.strips.height = Spread::of(&mut store.single_sizes.1);
    for s in &skipped {
        *report.skips.entry(s.reason.label().to_owned()).or_insert(0) += 1;
    }
    report.overlays = overlays.len();
    report.blocked_hexes = blocked.len();
    report.set_contributors(store.contributors(), config.top_contributors);

    placements.sort_by(|a, b| {
        a.hex
            .cmp(&b.hex)
            .then_with(|| a.reference.cmp(&b.reference))
            .then(a.art.cmp(&b.art))
    });
    skipped.sort();
    report.check()?;
    Ok(Sliced {
        art: store.art,
        placements,
        overlays,
        blocked_hexes: blocked.into_iter().collect(),
        skipped,
        report,
    })
}

/// [`estimate`] over any [`Assets`].
pub fn estimate_with(
    assets: &dyn Assets,
    references: &[Reference],
    window: &MapWindow,
    _sampler: &GroundSampler,
    projection: &Projection,
    budget: &Budget,
    config: &SlicerConfig,
) -> Result<BudgetReport, SliceError> {
    config.validate()?;
    let plane =
        Plane::new(projection, window.x_west, window.y_north).ok_or(SliceError::Projection)?;
    let tile_units = projection.tile_units();
    let prepared = prepare(assets, references, tile_units, config);
    let mut report = BudgetReport::new(true, budget);
    let mut sprite_keys: BTreeSet<(ArtCategory, RenderKey)> = BTreeSet::new();
    let mut renders: BTreeMap<ClassKind, BTreeSet<RenderKey>> = BTreeMap::new();
    let mut contributions: BTreeMap<(ArtCategory, String, ClassKind), (usize, usize)> =
        BTreeMap::new();
    for (r, class) in prepared.refs.iter().zip(&prepared.classes) {
        let kind = class.kind();
        report.class_mut(kind).references += 1;
        let model = r.model.clone().unwrap_or_default();
        match class {
            Class::Skip(reason) => {
                *report.skips.entry(reason.label().to_owned()).or_insert(0) += 1;
            }
            Class::GroundFlat | Class::GroundRelief => {
                report.class_mut(kind).renders += 1;
            }
            Class::Sprite(role) => {
                let Some(key) = RenderKey::of(r, config) else {
                    continue;
                };
                renders.entry(kind).or_default().insert(key.clone());
                let c = contributions
                    .entry((role.category(), model, kind))
                    .or_default();
                c.0 += 1;
                if sprite_keys.insert((role.category(), key)) {
                    c.1 += 1;
                    report.class_mut(kind).entries += 1;
                    report.category_mut(role.category()).used += 1;
                }
            }
            Class::Sliced => {
                let Some(key) = RenderKey::of(r, config) else {
                    continue;
                };
                let Some(Ok((_, facts))) = prepared.meshes.get(&key.mesh) else {
                    continue;
                };
                renders.entry(kind).or_default().insert(key);
                let Some(bounds) = facts.bounds else {
                    continue;
                };
                let footprint = Footprint::of(&r.transform, bounds);
                let polygon: Vec<[f64; 2]> = footprint
                    .polygon
                    .iter()
                    .map(|p| plane.tile_of_source(*p))
                    .collect();
                let hexes = plane.hexes_in(&polygon);
                let n = if hexes.is_empty() {
                    let hex_units = f64::from(tile_units) / 2.0;
                    (footprint.longest_side() / hex_units).ceil().max(1.0) as usize
                } else {
                    estimate_strips(&hexes, config.merge_interior_runs, config.max_perimeter_run)
                };
                let c = contributions
                    .entry((ArtCategory::Walls, model, kind))
                    .or_default();
                c.0 += 1;
                c.1 += n;
                report.class_mut(kind).entries += n;
                report.category_mut(ArtCategory::Walls).used += n;
                report.strips.placed += n;
            }
        }
    }
    for (kind, keys) in renders {
        report.class_mut(kind).renders = keys.len();
    }
    let all = contributions
        .into_iter()
        .map(
            |((category, model, class), (references, entries))| Contributor {
                category,
                model,
                class,
                references,
                entries,
            },
        )
        .collect();
    report.set_contributors(all, config.top_contributors);
    report.strips.distinct = report.category(ArtCategory::Walls).used;
    Ok(report)
}

/// Strips a footprint would give: row runs of interior hexes count once
/// (when merged), row runs of perimeter hexes once per piece of at most
/// `max_run`.
fn estimate_strips(hexes: &[i32], merge_interior: bool, max_run: usize) -> usize {
    let set: BTreeSet<i32> = hexes.iter().copied().collect();
    let interior = |h: i32| {
        hex::neighbours(h)
            .iter()
            .all(|n| n.is_some_and(|n| set.contains(&n)))
    };
    let kinds: Vec<bool> = hexes.iter().map(|&h| interior(h)).collect();
    let mut count = 0;
    let mut i = 0;
    while i < hexes.len() {
        let row = hex::row_column(hexes[i]).0;
        let mut j = i;
        while j + 1 < hexes.len()
            && hexes[j + 1] == hexes[j] + 1
            && hex::row_column(hexes[j + 1]).0 == row
            && kinds[j + 1] == kinds[i]
        {
            j += 1;
        }
        let len = j - i + 1;
        count += match (kinds[i], merge_interior) {
            (true, true) => 1,
            (true, false) => len,
            (false, _) => len.div_ceil(max_run.max(1)),
        };
        i = j + 1;
    }
    count
}
