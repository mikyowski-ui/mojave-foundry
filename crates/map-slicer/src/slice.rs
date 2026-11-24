//! The run: classify, render, cut, deduplicate, account.

use std::collections::{BTreeMap, BTreeSet};

use emit::PlacedFrame;
use foundry_render::{Projection, Renderer};
use mir::{GroundSampler, MapWindow, Mir, Reference, Transform};
use nif::StaticMesh;
use quantize::{Dither, PaletteMap};
use sha2::{Digest, Sha256};

use crate::classify::{Footprint, MeshFacts};
use crate::cutaway;
use crate::declutter::{self, DecalMode, Decision};
use crate::floor;
use crate::hex::{self, Plane};
use crate::interior::{self, Split};
use crate::landmark;
use crate::overlay::{self, GroundPiece};
use crate::render::{self, Assets, CanvasError, MirAssets};
use crate::report::{
    BudgetReport, Contributor, CutawayStats, InteriorStats, LadderKind, LadderStep,
    LandmarkFallback, Spread,
};
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
/// merged), landmarks by the pieces of their drawn extent's visibility cut
/// (sorting cuts are not foreseen, so a real run costs more). Meshes
/// are read for their bounds and triangle area. The budget ladder is not
/// climbed.
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
    /// Interior mode, for a render that is split and cut: the origin's
    /// height above the floor, rounded to a unit. `None` otherwise.
    lift: Option<i64>,
    /// Declutter, interior mode: a lower ceiling cut for a room shell,
    /// above the floor, rounded to a unit. `None`: the interior's own.
    cut: Option<i64>,
    /// The wire rule's look: bit 0 the wire cutoff, bit 1 both faces.
    look: u8,
    /// Declutter, outdoors: a digest of the roof triangles left out over
    /// walkable ground. `None`: none.
    roof: Option<u64>,
}

impl RenderKey {
    /// The key of a render split at the floor and cut at the ceiling, its
    /// origin `lift` units above the floor.
    pub(crate) fn lifted(self, lift: f32) -> Self {
        RenderKey {
            lift: Some(f64::from(lift).round() as i64),
            ..self
        }
    }

    /// The rounded lift of a split render.
    pub(crate) fn lift(&self) -> Option<f32> {
        self.lift.map(|v| v as f32)
    }

    /// The key of a render cut at `cut` above the floor instead of the
    /// interior's own height (`None` keeps that).
    pub(crate) fn with_cut(self, cut: Option<f32>) -> Self {
        RenderKey {
            cut: cut.map(|c| f64::from(c).round() as i64),
            ..self
        }
    }

    /// The rounded shell cut of a split render.
    pub(crate) fn cut(&self) -> Option<f32> {
        self.cut.map(|v| v as f32)
    }

    pub(crate) fn of(reference: &Reference, config: &SlicerConfig) -> Option<Self> {
        let q = |v: f32, step: f32| (f64::from(v) / f64::from(step)).round() as i64;
        Some(RenderKey {
            mesh: reference.mesh.clone()?,
            rotation: reference
                .transform
                .rotation
                .map(|a| q(a, config.rotation_quantum)),
            scale: q(reference.transform.scale, config.scale_quantum),
            lift: None,
            cut: None,
            look: 0,
            roof: None,
        })
    }

    /// The key of a render drawn with the wire rule's `look` bits.
    pub(crate) fn with_look(self, look: u8) -> Self {
        RenderKey { look, ..self }
    }

    /// The key of a render without the roof triangles whose digest is
    /// `roof`.
    pub(crate) fn with_roof(self, roof: Option<u64>) -> Self {
        RenderKey { roof, ..self }
    }

    /// How the render is drawn.
    pub(crate) fn look(&self, config: &SlicerConfig) -> render::Look {
        render::Look {
            two_sided: self.look & LOOK_TWO_SIDED != 0,
            alpha_cutoff: (self.look & LOOK_WIRE != 0)
                .then(|| config.wire.as_ref().map_or(0.1, |w| w.cutoff)),
        }
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

/// [`RenderKey`] look bits.
const LOOK_WIRE: u8 = 1;
const LOOK_TWO_SIDED: u8 = 2;

type MeshSlot = Result<(StaticMesh, MeshFacts), String>;

/// References sorted and classified, with their meshes.
struct Prepared<'r> {
    refs: Vec<&'r Reference>,
    classes: Vec<Class>,
    meshes: BTreeMap<String, MeshSlot>,
    /// The classes before the declutter pass (the baseline's).
    plain: Vec<Class>,
    /// The declutter pass's decisions, when it is on.
    decision: Option<Decision>,
    /// Floor square side, source units.
    tile_units: f32,
}

impl Prepared<'_> {
    /// Whether the declutter pass protects reference `i`.
    fn protected(&self, i: usize) -> bool {
        self.decision.as_ref().is_some_and(|d| d.protected[i])
    }

    /// Runs the declutter pass's classification rules (the `declutter`
    /// module) when `config` turns it on.
    fn declutter(
        &mut self,
        plane: &Plane,
        projection: &Projection,
        window: &MapWindow,
        config: &SlicerConfig,
    ) {
        let Some(profile) = &config.declutter else {
            return;
        };
        let meshes = &self.meshes;
        let facts = |r: &Reference| {
            r.mesh
                .as_ref()
                .and_then(|h| meshes.get(h))
                .and_then(|m| m.as_ref().ok())
                .map(|(_, f)| *f)
        };
        let decision = declutter::decide(
            &self.refs,
            &mut self.classes,
            &facts,
            projection.tile_units(),
            plane,
            projection,
            window,
            config,
            profile,
        );
        self.decision = Some(decision);
    }
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
        plain: classes.clone(),
        classes,
        meshes,
        decision: None,
        tile_units,
    }
}

/// Where a reference stands in map screen space: its ground point rounded
/// to a pixel, the rounded offset from there to its origin (its height),
/// and the terrain height under it.
#[cfg(test)]
pub(crate) fn standing(
    projection: &Projection,
    window: &MapWindow,
    sampler: &GroundSampler,
    reference: &Reference,
) -> Option<([i64; 2], [i32; 2], f32)> {
    standing_on(projection, window, sampler, None, reference)
}

/// [`standing`], on the common floor height `floor` when one is given
/// (interior mode) instead of the terrain.
pub(crate) fn standing_on(
    projection: &Projection,
    window: &MapWindow,
    sampler: &GroundSampler,
    floor: Option<f32>,
    reference: &Reference,
) -> Option<([i64; 2], [i32; 2], f32)> {
    let [x, y, z] = reference.transform.position;
    let ground_z = floor.unwrap_or_else(|| sampler.height(x, y).unwrap_or(z));
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

/// The model a skipped reference is listed under: its model path; for a
/// trigger volume whose base has none (the usual case: the volume is a
/// primitive), `base <editor id>` when the base has one.
fn skipped_model(r: &Reference, reason: &SkipReason) -> Option<String> {
    match (&r.model, reason) {
        (Some(model), _) => Some(model.clone()),
        (None, SkipReason::TriggerVolume) => {
            r.base_editor_id.as_ref().map(|id| format!("base {id}"))
        }
        (None, _) => None,
    }
}

/// A skip found after classification moves its reference out of its class.
fn demote(report: &mut BudgetReport, kind: ClassKind) {
    report.class_mut(kind).references -= 1;
    report.class_mut(ClassKind::Skipped).references += 1;
}

/// Distance from a point to a convex polygon (0 inside).
pub(crate) fn distance_to_polygon(polygon: &[[f64; 2]], p: [f64; 2]) -> f64 {
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

/// Whether a reference of this class is split at the floor and cut at the
/// ceiling indoors: everything rendered upright but doors, containers and
/// carried items, which are drawn whole.
fn splits(class: &Class) -> bool {
    !matches!(
        class,
        Class::Sprite(Role::Item | Role::Door | Role::Container)
    )
}

/// A strip blocks when its hex centre is within this many floor squares of
/// the object's footprint: about half a hex, so a thin fence still blocks
/// the hexes it crosses while eaves over open ground do not.
pub(crate) const BLOCK_REACH: f64 = 0.3;

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
    let mut prepared = prepare(assets, references, tile_units, config);
    prepared.declutter(&plane, projection, window, config);
    let inputs = Inputs {
        assets,
        window,
        sampler,
        projection,
        map,
        budget,
        plane: &plane,
    };
    let first = run_once(&inputs, renderer, &prepared, &prepared.classes, config)?;
    let mut done = if first.sliced.report.fits() || !config.ladder.enabled {
        first
    } else {
        climb(&inputs, renderer, &prepared, config, first)?
    };
    if let Some(profile) = &config.declutter {
        measure(&inputs, renderer, &prepared, config, profile, &mut done)?;
    }
    done.sliced.report.check()?;
    Ok(done.sliced)
}

/// The declutter pass's floor metrics: this run's, and with
/// `measure_baseline` the same window's without the pass (its classes
/// before the pass, no ladder).
fn measure(
    inputs: &Inputs<'_>,
    renderer: &mut dyn Renderer,
    prepared: &Prepared<'_>,
    config: &SlicerConfig,
    profile: &declutter::DeclutterProfile,
    done: &mut Run,
) -> Result<(), SliceError> {
    let walkers = profile.walkers_of(config);
    let mut floor = walkers.clone();
    if let Some(i) = &done.sliced.report.interior {
        floor.extend(i.floor_hexes.iter().copied());
    }
    let stats = |sliced: &Sliced| {
        declutter::floor_stats(
            sliced,
            &walkers,
            &floor,
            config.walker_box,
            profile.cover_min_pixels,
        )
    };
    let after = stats(&done.sliced);
    let before = if profile.measure_baseline {
        let plain = SlicerConfig {
            declutter: None,
            ..config.clone()
        };
        let baseline = run_once(inputs, renderer, prepared, &prepared.plain, &plain)?;
        Some(stats(&baseline.sliced))
    } else {
        None
    };
    if let Some(d) = &mut done.sliced.report.declutter {
        d.floor_after = Some(after);
        d.floor_before = before;
    }
    Ok(())
}

/// What every run of one window shares.
struct Inputs<'a> {
    assets: &'a dyn Assets,
    window: &'a MapWindow,
    sampler: &'a GroundSampler,
    projection: &'a Projection,
    map: &'a PaletteMap,
    budget: &'a Budget,
    plane: &'a Plane,
}

/// One run's result, with every contributor (the report keeps the top).
struct Run {
    sliced: Sliced,
    contributors: Vec<Contributor>,
}

impl Run {
    fn used(&self) -> [usize; 3] {
        self.sliced.report.used()
    }

    /// How far a list is over its free entries (negative: room left).
    fn over(&self, category: ArtCategory) -> i64 {
        -self.sliced.report.category(category).headroom()
    }
}

/// The model key contributors are counted under.
fn model_of(reference: &Reference) -> String {
    reference.model.clone().unwrap_or_default()
}

/// Scenery entries a reference would cost as a landmark: its bounding
/// box's drawn extent in pieces of the visibility cut (an estimate; sorting
/// may cut more, whole-hex copies share art).
fn landmark_estimate(
    prepared: &Prepared<'_>,
    i: usize,
    projection: &Projection,
    config: &SlicerConfig,
) -> usize {
    let r = prepared.refs[i];
    let Some(Some(Ok((_, facts)))) = r.mesh.as_ref().map(|h| prepared.meshes.get(h)) else {
        return 1;
    };
    let Some(bounds) = facts.bounds else {
        return 1;
    };
    let m = r.transform.matrix();
    let anchor = r.transform.position;
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for corner in 0..8 {
        let local = [
            bounds[corner & 1][0],
            bounds[(corner >> 1) & 1][1],
            bounds[(corner >> 2) & 1][2],
        ];
        let p = [0, 1, 2].map(|row| {
            m[row][0] * local[0] + m[row][1] * local[1] + m[row][2] * local[2] + m[row][3]
        });
        let s = projection.screen(projection.tile_from_source(anchor, p));
        let (x, y) = (f64::from(s[0]), f64::from(s[1]));
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    if !b.iter().all(|v| v.is_finite()) {
        return 1;
    }
    landmark::estimate([b[2] - b[0], b[3] - b[1]], &landmark::Limits::of(config))
}

/// The budget ladder (crate documentation, "Budget"): the steps in order,
/// each only while a list is still over, re-running after each.
fn climb(
    inputs: &Inputs<'_>,
    renderer: &mut dyn Renderer,
    prepared: &Prepared<'_>,
    config: &SlicerConfig,
    first: Run,
) -> Result<Run, SliceError> {
    let ladder = &config.ladder;
    let mut config = config.clone();
    let mut classes = prepared.classes.clone();
    let mut run = first;
    let mut steps = Vec::new();
    let models: Vec<String> = prepared.refs.iter().map(|r| model_of(r)).collect();

    // 1 and 2: thresholds that only coarsen strips.
    if run.over(ArtCategory::Walls) > 0 && config.max_perimeter_run < ladder.perimeter_run {
        let before = run.used();
        let change = format!(
            "max_perimeter_run {} -> {}",
            config.max_perimeter_run, ladder.perimeter_run
        );
        config.max_perimeter_run = ladder.perimeter_run;
        run = run_once(inputs, renderer, prepared, &classes, &config)?;
        steps.push(LadderStep {
            kind: LadderKind::LongerRuns,
            change,
            models: Vec::new(),
            references: 0,
            before,
            after: run.used(),
        });
    }
    if run.over(ArtCategory::Walls) > 0 && config.min_strip_pixels < ladder.min_strip_pixels {
        let before = run.used();
        let change = format!(
            "min_strip_pixels {} -> {}",
            config.min_strip_pixels, ladder.min_strip_pixels
        );
        config.min_strip_pixels = ladder.min_strip_pixels;
        run = run_once(inputs, renderer, prepared, &classes, &config)?;
        steps.push(LadderStep {
            kind: LadderKind::CoarserStrips,
            change,
            models: Vec::new(),
            references: 0,
            before,
            after: run.used(),
        });
    }

    // 3: the largest sliced models become landmarks, as long as the
    // scenery list has room for their pieces.
    if run.over(ArtCategory::Walls) > 0 && config.landmarks && ladder.max_landmarks > 0 {
        let before = run.used();
        let mut promoted: BTreeSet<String> = BTreeSet::new();
        let mut moved = 0;
        for _ in 0..ladder.max_rounds {
            let mut need = run.over(ArtCategory::Walls);
            let mut room = -run.over(ArtCategory::Scenery);
            let mut candidates: Vec<&Contributor> = run
                .contributors
                .iter()
                .filter(|c| c.category == ArtCategory::Walls && c.class == ClassKind::Sliced)
                .filter(|c| !promoted.contains(&c.model))
                .collect();
            candidates.sort_by(|a, b| b.entries.cmp(&a.entries).then(a.model.cmp(&b.model)));
            let mut picked: Vec<(String, Vec<usize>)> = Vec::new();
            for c in candidates {
                if need <= 0 || promoted.len() + picked.len() >= ladder.max_landmarks {
                    break;
                }
                let members: Vec<usize> = (0..classes.len())
                    .filter(|&i| classes[i] == Class::Sliced && models[i] == c.model)
                    .collect();
                let cost: usize = members
                    .iter()
                    .map(|&i| landmark_estimate(prepared, i, inputs.projection, &config))
                    .sum();
                if members.is_empty() || cost >= c.entries || cost as i64 > room {
                    continue;
                }
                need -= (c.entries - cost) as i64;
                room -= cost as i64;
                picked.push((c.model.clone(), members));
            }
            if picked.is_empty() {
                break;
            }
            for (model, members) in picked {
                for i in members {
                    classes[i] = Class::Landmark;
                    moved += 1;
                }
                promoted.insert(model);
            }
            run = run_once(inputs, renderer, prepared, &classes, &config)?;
            if run.over(ArtCategory::Walls) <= 0 {
                break;
            }
        }
        if !promoted.is_empty() {
            steps.push(LadderStep {
                kind: LadderKind::Landmarks,
                change: format!("{} models", promoted.len()),
                models: promoted.into_iter().collect(),
                references: moved,
                before,
                after: run.used(),
            });
        }
    }

    // 4: the smallest contributors of each list still over go to the
    // ground. Doors, containers and items never do.
    if !run.sliced.report.fits() && ladder.demote {
        let before = run.used();
        let mut demoted: BTreeSet<String> = BTreeSet::new();
        let mut moved = 0;
        for _ in 0..ladder.max_rounds {
            let mut picked: Vec<(String, Vec<usize>)> = Vec::new();
            for category in [ArtCategory::Walls, ArtCategory::Scenery] {
                let mut need = run.over(category);
                if need <= 0 {
                    continue;
                }
                let demotable = |class: &Class| match category {
                    ArtCategory::Walls => *class == Class::Sliced,
                    _ => matches!(class, Class::Sprite(Role::Scenery) | Class::Landmark),
                };
                let mut candidates: Vec<&Contributor> = run
                    .contributors
                    .iter()
                    .filter(|c| c.category == category && c.entries > 0)
                    .filter(|c| match category {
                        ArtCategory::Walls => c.class == ClassKind::Sliced,
                        _ => matches!(c.class, ClassKind::Sprite | ClassKind::Landmark),
                    })
                    .filter(|c| !demoted.contains(&c.model))
                    .collect();
                candidates.sort_by(|a, b| a.entries.cmp(&b.entries).then(a.model.cmp(&b.model)));
                for c in candidates {
                    if need <= 0 {
                        break;
                    }
                    if picked.iter().any(|(m, _)| *m == c.model) {
                        continue;
                    }
                    // Protected references (the declutter pass) stay.
                    let members: Vec<usize> = (0..classes.len())
                        .filter(|&i| demotable(&classes[i]) && models[i] == c.model)
                        .filter(|&i| !prepared.protected(i))
                        .collect();
                    if members.is_empty() {
                        continue;
                    }
                    need -= c.entries as i64;
                    picked.push((c.model.clone(), members));
                }
            }
            if picked.is_empty() {
                break;
            }
            for (model, members) in picked {
                for i in members {
                    classes[i] = Class::GroundRelief;
                    moved += 1;
                }
                demoted.insert(model);
            }
            run = run_once(inputs, renderer, prepared, &classes, &config)?;
            if run.sliced.report.fits() {
                break;
            }
        }
        if !demoted.is_empty() {
            steps.push(LadderStep {
                kind: LadderKind::Demoted,
                change: format!("{} models to ground relief", demoted.len()),
                models: demoted.into_iter().collect(),
                references: moved,
                before,
                after: run.used(),
            });
        }
    }
    run.sliced.report.ladder = steps;
    Ok(run)
}

/// One pass over the window with the given classes: render, cut,
/// deduplicate, account. The budget is not checked here.
fn run_once(
    inputs: &Inputs<'_>,
    renderer: &mut dyn Renderer,
    prepared: &Prepared<'_>,
    classes: &[Class],
    config: &SlicerConfig,
) -> Result<Run, SliceError> {
    let Inputs {
        assets,
        window,
        sampler,
        projection,
        map,
        budget,
        plane,
    } = *inputs;
    // Interior mode (the `interior` module): no land, every height comes
    // from the floor.
    let interior = config.interior.as_ref();
    let no_land = GroundSampler::default();
    let sampler = if interior.is_some() {
        &no_land
    } else {
        sampler
    };
    let floor_z = interior.map(|i| i.floor_z);
    let mut indoor = interior.map(InteriorStats::new);
    // The declutter pass (the `declutter` module), when it is on.
    let decision = config.declutter.as_ref().zip(prepared.decision.as_ref());
    let walkers = decision.map(|(profile, _)| profile.walkers_of(config));
    let cutaway = decision.and_then(|(profile, _)| {
        profile.cutaway.as_ref().filter(|c| {
            if interior.is_some() {
                c.interiors
            } else {
                c.exteriors
            }
        })
    });
    let mut cut_stats = CutawayStats::default();
    let mut covered = (BTreeSet::new(), BTreeSet::new());
    let mut door_covered = (BTreeSet::new(), BTreeSet::new());
    let mut outlined = 0usize;
    let mut outlined_kinds: BTreeMap<String, usize> = BTreeMap::new();
    // Hexes of strips the cutaway cut (doors near them get the stub).
    let mut cut_hexes: BTreeSet<i32> = BTreeSet::new();
    let door_rule = cutaway.and_then(|c| c.doors.as_ref());
    let deferred = |i: usize| door_rule.is_some() && decision.is_some_and(|(_, d)| d.door_like[i]);
    let mut roof_stats: Option<(usize, usize)> = decision.map(|_| (0, 0));
    // Floor meshes split off renders, and (floor mesh, reference) to bake.
    let mut floor_meshes: Vec<StaticMesh> = Vec::new();
    let mut floor_bake: Vec<(usize, usize)> = Vec::new();
    let mut report = BudgetReport::new(false, budget);
    let mut skipped = Vec::new();
    let mut placements = Vec::new();
    let mut store = ArtStore::default();
    let mut blocked = BTreeSet::new();
    let mut fallbacks: Vec<LandmarkFallback> = Vec::new();
    let skip = |skipped: &mut Vec<Skipped>, r: &Reference, reason: SkipReason| {
        skipped.push(Skipped {
            reference: r.form.to_string(),
            model: skipped_model(r, &reason),
            reason,
        });
    };

    // The wire rule's looks, the roofs left out over walkable ground
    // outdoors, and the furniture (each off unless configured).
    let looks: Vec<u8> = {
        let mut test = WireTest {
            renderer: &mut *renderer,
            verdicts: BTreeMap::new(),
        };
        (0..classes.len())
            .map(|i| {
                look_of(
                    prepared,
                    i,
                    &classes[i],
                    config,
                    assets.textures(),
                    projection,
                    &mut test,
                )
            })
            .collect()
    };
    let roof_rule = decision
        .and_then(|(profile, _)| profile.roof_cut.as_ref())
        .filter(|_| interior.is_none());
    let mut roofs: BTreeMap<usize, u64> = BTreeMap::new();
    let mut roof_masks: BTreeMap<u64, Vec<Vec<bool>>> = BTreeMap::new();
    if let (Some(rule), Some(walkers)) = (roof_rule, &walkers) {
        for (i, class) in classes.iter().enumerate() {
            if !matches!(class, Class::Sliced | Class::Landmark) {
                continue;
            }
            let r = prepared.refs[i];
            let Some(Some(Ok((mesh, _)))) = r.mesh.as_ref().map(|h| prepared.meshes.get(h)) else {
                continue;
            };
            let mask = roof_mask(plane, sampler, mesh, &r.transform.matrix(), walkers, rule);
            let removed: usize = mask.iter().map(|m| m.iter().filter(|v| **v).count()).sum();
            if removed == 0 {
                continue;
            }
            let mut hasher = Sha256::new();
            for (p, m) in mask.iter().enumerate() {
                for (t, v) in m.iter().enumerate() {
                    if *v {
                        hasher.update((p as u64).to_le_bytes());
                        hasher.update((t as u64).to_le_bytes());
                    }
                }
            }
            let digest: [u8; 32] = hasher.finalize().into();
            let h = u64::from_le_bytes(digest[..8].try_into().unwrap_or([0; 8]));
            roofs.insert(i, h);
            roof_masks.insert(h, mask);
            if let Some(d) = &mut roof_stats {
                d.0 += 1;
                d.1 += removed;
            }
        }
    }
    let mut furniture_choice: BTreeMap<usize, &'static str> = match &config.furniture {
        Some(rule) => (0..classes.len())
            .filter_map(|i| {
                furniture_of(prepared, i, &classes[i], rule, config, sampler, decision)
                    .map(|why| (i, why))
            })
            .collect(),
        None => BTreeMap::new(),
    };
    let mut footprints: BTreeMap<String, Vec<[f32; 2]>> = BTreeMap::new();
    let mut body_total: BTreeSet<i32> = BTreeSet::new();
    let mut open_blocked: BTreeSet<i32> = BTreeSet::new();

    // Group the sprite, sliced and landmark references by render.
    let mut groups: BTreeMap<RenderKey, Vec<usize>> = BTreeMap::new();
    let mut flat = Vec::new();
    let mut relief = Vec::new();
    for (i, (r, class)) in prepared.refs.iter().zip(classes).enumerate() {
        report.class_mut(class.kind()).references += 1;
        match class {
            Class::Skip(reason) => skip(&mut skipped, r, reason.clone()),
            Class::GroundFlat => flat.push(i),
            Class::GroundRelief => relief.push(i),
            Class::Sprite(_) | Class::Sliced | Class::Landmark => match RenderKey::of(r, config) {
                Some(_)
                    if looks[i] & LOOK_WIRE != 0
                        && config.wire.as_ref().is_some_and(|w| w.drop) =>
                {
                    report.wire_props += 1;
                    *report.wire_models.entry(model_of(r)).or_insert(0) += 1;
                    skip(&mut skipped, r, SkipReason::Wire);
                    demote(&mut report, class.kind());
                }
                Some(key) => {
                    let look = looks[i];
                    report.wire_props += usize::from(look & LOOK_WIRE != 0);
                    if look & LOOK_WIRE != 0 {
                        *report.wire_models.entry(model_of(r)).or_insert(0) += 1;
                    }
                    report.two_sided += usize::from(look & LOOK_TWO_SIDED != 0);
                    // Wire props are drawn whole: none of their lattice is
                    // floor.
                    let key = match interior {
                        Some(indoors) if splits(class) && look & LOOK_WIRE == 0 => key
                            .lifted(r.transform.position[2] - indoors.floor_z)
                            .with_cut(decision.and_then(|(_, d)| d.cuts[i])),
                        _ => key,
                    };
                    let key = key.with_look(look).with_roof(roofs.get(&i).copied());
                    groups.entry(key).or_default().push(i)
                }
                None => skip(&mut skipped, r, SkipReason::NoMesh),
            },
        }
    }

    let textures = assets.textures();
    let near_open = floor::around(&config.open_hexes, config.floor_reach);
    let mut strip_runs = 0usize;
    // A landmark that cannot be drawn upright is baked as relief instead.
    let mut fall_back =
        |i: usize, reason: String, report: &mut BudgetReport, relief: &mut Vec<usize>| {
            let r = prepared.refs[i];
            report.class_mut(ClassKind::Landmark).references -= 1;
            report.class_mut(ClassKind::GroundRelief).references += 1;
            relief.push(i);
            fallbacks.push(LandmarkFallback {
                reference: r.form.to_string(),
                model: model_of(r),
                reason,
            });
        };
    // Doors the cutaway's door rule may crop come second, once every wall
    // strip is cut.
    for pass in 0..2u8 {
        for (key, all) in &groups {
            let members: Vec<usize> = all
                .iter()
                .copied()
                .filter(|&i| deferred(i) == (pass == 1))
                .collect();
            if members.is_empty() {
                continue;
            }
            let members = &members;
            let Some(Ok((mesh, _))) = prepared.meshes.get(&key.mesh) else {
                continue;
            };
            let roofless;
            let mesh: &StaticMesh = match key.roof.and_then(|h| roof_masks.get(&h)) {
                Some(mask) => {
                    roofless = without_triangles(mesh, mask);
                    &roofless
                }
                None => mesh,
            };
            let first_kind = classes[members[0]].kind();
            // Indoors: the floor leaves for the bake, the rest is cut at the
            // ceiling, and only that is rendered upright.
            let mut split = match (key.lift(), interior) {
                (Some(lift), Some(indoors)) => Some(interior::split_at(
                    mesh,
                    &key.matrix(config),
                    lift,
                    indoors,
                    config.floor_min_up,
                    key.cut().unwrap_or(indoors.cut_height),
                )),
                _ => None,
            };
            let floor_index = split.as_mut().and_then(|s| s.floor.take()).map(|f| {
                floor_meshes.push(f);
                floor_meshes.len() - 1
            });
            let clipped = split.as_ref().is_some_and(|s| s.cut_triangles > 0);
            if let (Some(s), Some(t)) = (&split, &mut indoor) {
                t.cut_triangles += s.cut_triangles;
                t.ceiling_triangles += s.ceiling_triangles;
            }
            // A placed member gives its floor to the bake.
            let placed =
                |i: usize, indoor: &mut Option<InteriorStats>, bake: &mut Vec<(usize, usize)>| {
                    if let Some(f) = floor_index {
                        bake.push((f, i));
                    }
                    if let Some(t) = indoor
                        && clipped
                    {
                        t.clipped_references += 1;
                    }
                };
            // Members left with nothing upright: their floor is baked (when
            // they stand on the map); without one, nothing is under the cut.
            let floor_only = |skipped: &mut Vec<Skipped>,
                              report: &mut BudgetReport,
                              indoor: &mut Option<InteriorStats>,
                              bake: &mut Vec<(usize, usize)>| {
                for &i in members {
                    let r = prepared.refs[i];
                    if floor_index.is_none() {
                        skip(skipped, r, SkipReason::AboveCut);
                        demote(report, classes[i].kind());
                    } else if standing_on(projection, window, sampler, floor_z, r).is_none() {
                        skip(skipped, r, SkipReason::OutsideMap);
                        demote(report, classes[i].kind());
                    } else {
                        placed(i, indoor, bake);
                        if let Some(t) = indoor {
                            t.floor_only += 1;
                        }
                    }
                }
            };
            let mesh: &StaticMesh = match &split {
                None => mesh,
                Some(Split {
                    walls: Some(walls), ..
                }) => walls,
                Some(_) => {
                    floor_only(&mut skipped, &mut report, &mut indoor, &mut floor_bake);
                    continue;
                }
            };
            // Landmarks fall back to relief when `relief_ok`; the rest skip.
            let mut fail_all = |reason: SkipReason,
                                relief_ok: bool,
                                skipped: &mut Vec<Skipped>,
                                report: &mut BudgetReport,
                                relief: &mut Vec<usize>| {
                for &i in members {
                    if relief_ok && classes[i] == Class::Landmark {
                        fall_back(i, reason.to_string(), report, relief);
                    } else {
                        skip(skipped, prepared.refs[i], reason.clone());
                        demote(report, classes[i].kind());
                    }
                }
            };
            let mut canvas = match render::render_canonical_with(
                renderer,
                projection,
                mesh,
                key.matrix(config),
                textures,
                config.render_tile,
                config.max_canvas_pixels,
                key.look(config),
            ) {
                Ok(Some(canvas)) => canvas,
                Ok(None) if floor_index.is_some() => {
                    floor_only(&mut skipped, &mut report, &mut indoor, &mut floor_bake);
                    continue;
                }
                Ok(None) => {
                    fail_all(
                        SkipReason::NoGeometry,
                        false,
                        &mut skipped,
                        &mut report,
                        &mut relief,
                    );
                    continue;
                }
                Err(CanvasError::TooLarge) => {
                    fail_all(
                        SkipReason::TooLarge,
                        true,
                        &mut skipped,
                        &mut report,
                        &mut relief,
                    );
                    continue;
                }
                Err(CanvasError::Render(e)) => {
                    fail_all(
                        SkipReason::Failed(format!("render: {e}")),
                        true,
                        &mut skipped,
                        &mut report,
                        &mut relief,
                    );
                    continue;
                }
            };
            if let Some((profile, _)) = decision {
                profile.tones.props.apply(&mut canvas.rgba);
            }
            if !(0..canvas.width as usize * canvas.height as usize).any(|i| canvas.opaque(i)) {
                if floor_index.is_some() {
                    floor_only(&mut skipped, &mut report, &mut indoor, &mut floor_bake);
                    continue;
                }
                fail_all(
                    SkipReason::EmptyRender,
                    false,
                    &mut skipped,
                    &mut report,
                    &mut relief,
                );
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
                        true,
                        &mut skipped,
                        &mut report,
                        &mut relief,
                    );
                    continue;
                }
            };
            report.class_mut(first_kind).renders += 1;
            let mut sprite_frame: Option<Result<PlacedFrame, String>> = None;
            let mut outlined_frame: Option<Result<PlacedFrame, String>> = None;

            for &i in members {
                let r = prepared.refs[i];
                let class = &classes[i];
                let model = model_of(r);
                let Some((ground, offset, ground_z)) =
                    standing_on(projection, window, sampler, floor_z, r)
                else {
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
                        let rim = decision
                            .filter(|(_, d)| d.outlined[i])
                            .and_then(|(profile, _)| profile.outline);
                        // The door rule: a door in a cut wall that hides
                        // someone keeps a stub (the `cutaway` module).
                        let mut door_frame: Option<Result<PlacedFrame, String>> = None;
                        if let (true, Some(stub), Some(settings), Some(walkers)) =
                            (pass == 1, door_rule, cutaway, &walkers)
                        {
                            cut_stats.doors_considered += 1;
                            let near = cut_hexes
                                .range(h.saturating_sub(400 * (stub.reach + 1))..)
                                .any(|&c| hex::distance(c, h) <= stub.reach);
                            if !near {
                                cut_stats.doors_outside_cut += 1;
                            } else {
                                let foot = prepared
                                    .meshes
                                    .get(&key.mesh)
                                    .and_then(|m| m.as_ref().ok())
                                    .and_then(|(_, facts)| facts.bounds)
                                    .map(|b| Footprint::of(&r.transform, b).z[0]);
                                let floor = match interior {
                                    Some(indoors) => ground_z + indoors.floor_band[0],
                                    None => ground_z,
                                };
                                let trim = cutaway::Trim {
                                    settings,
                                    walkers,
                                    walker_box: config.walker_box.map(i64::from),
                                    offset,
                                    lift: r.transform.position[2]
                                        - foot.map_or(floor, |z| z.max(floor)),
                                };
                                let cut = cutaway::trim_door(
                                    &canvas,
                                    h,
                                    ground,
                                    projection,
                                    &trim,
                                    stub.stub_height,
                                    r.teleport.is_some().then_some(stub.min_click_pixels),
                                );
                                door_covered.0.extend(cut.before.iter().copied());
                                door_covered.1.extend(cut.after.iter().copied());
                                if cut.before.len() <= settings.max_walkers {
                                    cut_stats.doors_clear += 1;
                                } else if !cut.cleared.is_empty() {
                                    cut_stats.doors_trimmed += 1;
                                    cut_stats.doors_grown += usize::from(cut.grown);
                                    cut_stats.door_pixels_removed += cut.cleared.len() as u64;
                                    let mut stubbed = indexed.clone();
                                    for &c in &cut.cleared {
                                        stubbed[c] = 0;
                                    }
                                    door_frame = Some(match rim {
                                        None => emit::placed_frame(
                                            &stubbed,
                                            canvas.width,
                                            canvas.height,
                                            canvas.origin,
                                        )
                                        .map_err(|e| e.to_string()),
                                        Some(rim) => quantize::rim(
                                            &stubbed,
                                            canvas.width,
                                            canvas.height,
                                            map,
                                            &rim,
                                        )
                                        .map_err(|e| e.to_string())
                                        .and_then(|rimmed| {
                                            let origin = [
                                                canvas.origin[0] + rimmed.offset[0],
                                                canvas.origin[1] + rimmed.offset[1],
                                            ];
                                            emit::placed_frame(
                                                &rimmed.pixels,
                                                rimmed.width,
                                                rimmed.height,
                                                origin,
                                            )
                                            .map_err(|e| e.to_string())
                                        }),
                                    });
                                } else {
                                    cut_stats.doors_grown += usize::from(cut.grown);
                                }
                            }
                        }
                        let frame = match (&door_frame, rim) {
                            (Some(frame), _) => frame,
                            (None, None) => sprite_frame.get_or_insert_with(|| {
                                emit::placed_frame(
                                    &indexed,
                                    canvas.width,
                                    canvas.height,
                                    canvas.origin,
                                )
                                .map_err(|e| e.to_string())
                            }),
                            (None, Some(rim)) => outlined_frame.get_or_insert_with(|| {
                                let rimmed =
                                    quantize::rim(&indexed, canvas.width, canvas.height, map, &rim)
                                        .map_err(|e| e.to_string())?;
                                let origin = [
                                    canvas.origin[0] + rimmed.offset[0],
                                    canvas.origin[1] + rimmed.offset[1],
                                ];
                                emit::placed_frame(
                                    &rimmed.pixels,
                                    rimmed.width,
                                    rimmed.height,
                                    origin,
                                )
                                .map_err(|e| e.to_string())
                            }),
                        };
                        if rim.is_some() && frame.is_ok() {
                            outlined += 1;
                            *outlined_kinds
                                .entry(r.base_kind.clone().unwrap_or_else(|| "?".to_owned()))
                                .or_insert(0) += 1;
                        }
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
                        if config.furniture.as_ref().is_some_and(|f| f.footprints)
                            && furniture_choice
                                .get(&i)
                                .is_some_and(|why| is_furniture(why))
                            && let Some(polygon) = footprint_of(prepared, i)
                        {
                            footprints.insert(r.form.to_string(), polygon);
                        }
                        placed(i, &mut indoor, &mut floor_bake);
                    }
                    Class::Landmark => {
                        // Its footprint blocks (see the `landmark` module),
                        // except where the source lets an actor stand.
                        let mut under = BTreeSet::new();
                        if interior.is_some() {
                            interior::blocked_by_walls(
                                plane,
                                mesh,
                                &r.transform.matrix(),
                                ground_z,
                                config.step_height,
                                BLOCK_REACH,
                                &mut under,
                            );
                        } else {
                            overlay::blocked_under(
                                plane,
                                sampler,
                                mesh,
                                &r.transform.matrix(),
                                ground_z,
                                config.step_height,
                                &mut under,
                            );
                        }
                        let input = landmark::Input {
                            canvas: &canvas,
                            indexed: &indexed,
                            ground,
                            offset,
                            lift: r.transform.position[2] - ground_z,
                            projection,
                            under: &under,
                            open: &config.open_hexes,
                            walkable: config.landmark_walkers.as_ref(),
                            step_height: config.step_height,
                            crop_unreachable: false,
                        };
                        let limits = landmark::Limits::of(config);
                        // A landmark with pieces beyond the engine's reach is
                        // cut again without them, rather than flattened whole.
                        let cut = match landmark::cut(&input, &limits) {
                            Err(e)
                                if config.landmark_crop_unreachable
                                    && e.starts_with("no hex near") =>
                            {
                                landmark::cut(
                                    &landmark::Input {
                                        crop_unreachable: true,
                                        ..input
                                    },
                                    &limits,
                                )
                                .map_err(|again| format!("{e}; cropped: {again}"))
                            }
                            other => other,
                        };
                        let cut = match cut {
                            Ok(cut) => cut,
                            Err(e) => {
                                fall_back(i, format!("pieces: {e}"), &mut report, &mut relief);
                                continue;
                            }
                        };
                        if cut.stats.unreachable > 0 {
                            report
                                .landmarks
                                .unreachable
                                .push(crate::report::LandmarkCrop {
                                    reference: r.form.to_string(),
                                    model: model.clone(),
                                    pixels: cut.stats.unreachable,
                                    kept: cut.stats.reachable,
                                });
                        }
                        if cut.strips.is_empty() {
                            skip(&mut skipped, r, SkipReason::OutsideMap);
                            demote(&mut report, class.kind());
                            continue;
                        }
                        report.strips.dropped_pixels += cut.dropped;
                        report.landmarks.add(&cut.stats);
                        let mut under = cut.blocked;
                        store.note_reference(ArtCategory::Scenery, &model, ClassKind::Landmark);
                        for piece in &cut.strips {
                            let (art, new) = store.add(
                                ArtCategory::Scenery,
                                &piece.frame,
                                &model,
                                ClassKind::Landmark,
                            );
                            if new {
                                report.class_mut(ClassKind::Landmark).entries += 1;
                            }
                            let blocking = under.remove(&piece.hex);
                            placements.push(Placement {
                                art,
                                hex: piece.hex,
                                displacement: offset,
                                role: Role::Scenery,
                                reference: r.form.to_string(),
                                blocking,
                                run: None,
                            });
                            report.landmarks.pieces += 1;
                        }
                        blocked.extend(under);
                        report.landmarks.references += 1;
                        placed(i, &mut indoor, &mut floor_bake);
                    }
                    Class::Sliced => {
                        let footprint = prepared
                            .meshes
                            .get(&key.mesh)
                            .and_then(|m| m.as_ref().ok())
                            .and_then(|(_, facts)| facts.bounds)
                            .map(|b| Footprint::of(&r.transform, b));
                        let polygon: Vec<[f64; 2]> = footprint
                            .as_ref()
                            .map(|f| f.polygon.as_slice())
                            .unwrap_or_default()
                            .iter()
                            .map(|p| plane.tile_of_source(*p))
                            .collect();
                        // Furniture (the `furniture` module): its body blocks,
                        // open hex or not.
                        let furniture = config.furniture.as_ref().filter(|_| {
                            furniture_choice
                                .get(&i)
                                .is_some_and(|why| is_furniture(why))
                        });
                        let body: BTreeSet<i32> = match furniture {
                            Some(rule) => {
                                let mut body = BTreeSet::new();
                                crate::furniture::body_hexes(
                                    plane,
                                    mesh,
                                    &r.transform.matrix(),
                                    ground_z,
                                    config.step_height,
                                    rule.head_height,
                                    rule.reach,
                                    &mut body,
                                );
                                body
                            }
                            None => BTreeSet::new(),
                        };
                        let body_blocks = furniture.is_some_and(|f| f.block_open);
                        let blocks = |h: i32| {
                            (!config.open_hexes.contains(&h) || (body_blocks && body.contains(&h)))
                                && plane.hex_tile(h).is_some_and(|c| {
                                    distance_to_polygon(&polygon, c) <= BLOCK_REACH
                                })
                        };
                        if let Some(rule) = furniture {
                            // Its footprint, when its body fills its box.
                            let inside: BTreeSet<i32> =
                                plane.hexes_in(&polygon).into_iter().collect();
                            let fill = if inside.is_empty() {
                                if body.is_empty() { 0.0 } else { 1.0 }
                            } else {
                                inside.intersection(&body).count() as f64 / inside.len() as f64
                            };
                            if fill < rule.min_fill {
                                furniture_choice.insert(i, "hollow");
                            } else if rule.footprints
                                && let Some(polygon) = footprint_of(prepared, i)
                            {
                                footprints.insert(r.form.to_string(), polygon);
                            }
                            if rule.block_open {
                                open_blocked.extend(
                                    body.iter()
                                        .copied()
                                        .filter(|h| config.open_hexes.contains(h)),
                                );
                                blocked.extend(body.iter().copied());
                            }
                            body_total.extend(body.iter().copied());
                        }
                        // Walking surfaces by an open hex leave the strips (see
                        // the `floor` module). Indoors the floor is baked
                        // instead, and higher surfaces stay with the object.
                        let has_open = interior.is_none()
                            && config.open_hexes.iter().any(|&h| {
                                plane.hex_tile(h).is_some_and(|c| {
                                    distance_to_polygon(&polygon, c) <= BLOCK_REACH
                                })
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
                        // The foreground cutaway crops the strips of walls that
                        // hide walkers (the `cutaway` module).
                        let trim = match (cutaway, &walkers) {
                            (Some(settings), Some(walkers))
                                if decision.is_some_and(|(_, d)| d.cutaway[i]) =>
                            {
                                Some(cutaway::Trim {
                                    settings,
                                    walkers,
                                    walker_box: config.walker_box.map(i64::from),
                                    offset,
                                    // The kerb stands on the wall's own foot
                                    // (a raised ring, a sunken pit), never
                                    // below the floor band or the terrain.
                                    lift: r.transform.position[2]
                                        - footprint.as_ref().map_or(ground_z, |f| {
                                            f.z[0].max(match interior {
                                                Some(indoors) => ground_z + indoors.floor_band[0],
                                                None => ground_z,
                                            })
                                        }),
                                })
                            }
                            _ => None,
                        };
                        let mut trimmed = None;
                        let cut = match &trim {
                            None => strips::cut(
                                strip_canvas,
                                strip_indexed,
                                ground,
                                projection,
                                &options,
                            ),
                            Some(t) => strips::cut_with(
                                strip_canvas,
                                strip_indexed,
                                ground,
                                projection,
                                &options,
                                &mut |owner| {
                                    trimmed = Some(cutaway::trim(
                                        strip_canvas,
                                        owner,
                                        ground,
                                        projection,
                                        t,
                                    ));
                                },
                            ),
                        };
                        let emptied = trimmed.as_ref().is_some_and(|t| !t.emptied.is_empty());
                        if let Some(t) = trimmed {
                            cut_stats.strips_considered += t.considered;
                            cut_stats.strips_cut += t.cut;
                            cut_stats.pixels_removed += t.pixels_removed;
                            cut_stats.strips_emptied += t.emptied.len();
                            covered.0.extend(t.covered_before);
                            covered.1.extend(t.covered_after);
                            cut_hexes.extend(t.cut_hexes.iter().copied());
                            // A strip cut to nothing still blocks its hex.
                            blocked.extend(t.emptied.into_iter().filter(|&h| blocks(h)));
                        }
                        let cut = match cut {
                            Ok(cut) => cut,
                            Err(e) => {
                                skip(&mut skipped, r, SkipReason::Failed(e));
                                demote(&mut report, class.kind());
                                continue;
                            }
                        };
                        report.strips.dropped_pixels += cut.dropped;
                        if cut.strips.is_empty() && floor_pieces.is_empty() && !emptied {
                            if floor_index.is_some() {
                                // Indoors, its walls are off the map but its
                                // floor may not be.
                                placed(i, &mut indoor, &mut floor_bake);
                                if let Some(t) = &mut indoor {
                                    t.floor_only += 1;
                                }
                            } else {
                                skip(&mut skipped, r, SkipReason::OutsideMap);
                                demote(&mut report, class.kind());
                            }
                            continue;
                        }
                        report.strips.merged_tiny += cut.merged_tiny;
                        report.strips.folded += cut.folded;
                        blocked.extend(cut.run_hexes.iter().copied());
                        store.note_reference(ArtCategory::Walls, &model, ClassKind::Sliced);
                        for strip in &cut.strips {
                            let (art, new) = store.add(
                                ArtCategory::Walls,
                                &strip.frame,
                                &model,
                                ClassKind::Sliced,
                            );
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
                        placed(i, &mut indoor, &mut floor_bake);
                    }
                    _ => {}
                }
            }
        }
    }
    report.strips.runs = strip_runs;
    // Fallbacks join the relief in reference order.
    relief.sort_unstable();
    fallbacks.sort();
    report.landmarks.fallbacks = fallbacks;

    // Indoors, relief is split and cut like everything else: its walls in
    // relief, its floor to the bake.
    let mut relief_walls: BTreeMap<usize, StaticMesh> = BTreeMap::new();
    if let Some(indoors) = interior {
        relief.retain(|&i| {
            let r = prepared.refs[i];
            let Some(Some(Ok((mesh, _)))) = r.mesh.as_ref().map(|h| prepared.meshes.get(h)) else {
                return true;
            };
            let Some(key) = RenderKey::of(r, config) else {
                return true;
            };
            let lift = r.transform.position[2] - indoors.floor_z;
            let cut = decision.and_then(|(_, d)| d.cuts[i]);
            let s = interior::split_at(
                mesh,
                &key.matrix(config),
                lift,
                indoors,
                config.floor_min_up,
                cut.unwrap_or(indoors.cut_height),
            );
            if let Some(t) = &mut indoor {
                t.cut_triangles += s.cut_triangles;
                t.ceiling_triangles += s.ceiling_triangles;
                t.clipped_references += usize::from(s.cut_triangles > 0);
                t.floor_only += usize::from(s.walls.is_none() && s.floor.is_some());
            }
            if s.walls.is_none() && s.floor.is_none() {
                skip(&mut skipped, r, SkipReason::AboveCut);
                demote(&mut report, ClassKind::GroundRelief);
                return false;
            }
            if let Some(f) = s.floor {
                floor_meshes.push(f);
                floor_bake.push((floor_meshes.len() - 1, i));
            }
            match s.walls {
                Some(walls) => {
                    relief_walls.insert(i, walls);
                    true
                }
                None => false,
            }
        });
    }

    // Ground overlays: the interior floor, then flat, then relief.
    let anchor = [window.x_west, window.y_north, 0.0];
    let mut overlays = Vec::new();
    if let (Some(floor), Some(t)) = (floor_z, &mut indoor)
        && !floor_bake.is_empty()
    {
        floor_bake.sort_unstable_by_key(|&(_, i)| i);
        let mut hexes = BTreeSet::new();
        let mut pieces = Vec::with_capacity(floor_bake.len());
        for &(f, i) in &floor_bake {
            let mesh = &floor_meshes[f];
            let placed = prepared.refs[i].transform.matrix();
            t.floor_area += interior::floor_cover(plane, mesh, &placed, &mut hexes);
            t.floor_triangles += mesh
                .primitives
                .iter()
                .map(|p| p.indices.len() / 3)
                .sum::<usize>();
            let mut transform = placed;
            transform[2][3] -= floor;
            pieces.push(GroundPiece { mesh, transform });
        }
        let baked = overlay::bake(
            renderer,
            projection,
            anchor,
            &pieces,
            textures,
            config.render_tile,
        )?;
        t.floor_references = floor_bake.len();
        t.floor_overlays = baked.len();
        t.floor_hexes = hexes.into_iter().collect();
        let mut baked = baked;
        if let Some((profile, _)) = decision {
            for o in &mut baked {
                profile.tones.floor.apply(&mut o.rgba);
            }
        }
        overlays.extend(baked);
    }
    // Declutter: faded decals bake on their own, after the other flat
    // meshes, with their alpha scaled.
    let mut faded = Vec::new();
    if let Some((profile, d)) = decision
        && let DecalMode::Fade(_) = profile.decals
    {
        flat.retain(|&i| {
            if d.faded[i] {
                faded.push(i);
            }
            !d.faded[i]
        });
    }
    let fade = match decision.map(|(profile, _)| profile.decals) {
        Some(DecalMode::Fade(alpha)) => alpha,
        _ => 1.0,
    };
    let mut bakes = vec![(&flat, true, false), (&relief, false, false)];
    if !faded.is_empty() {
        bakes.insert(1, (&faded, true, true));
    }
    for (members, flatten, fading) in bakes {
        let mut pieces = Vec::new();
        for &i in members {
            let r = prepared.refs[i];
            let Some(Some(Ok((mesh, _)))) = r.mesh.as_ref().map(|h| prepared.meshes.get(h)) else {
                continue;
            };
            let mesh = match relief_walls.get(&i) {
                Some(walls) if !flatten => walls,
                _ => mesh,
            };
            let [x, y, z] = r.transform.position;
            let ground_z = floor_z.unwrap_or_else(|| sampler.height(x, y).unwrap_or(z));
            let mut transform = r.transform.matrix();
            transform[2][3] -= ground_z;
            if flatten {
                transform[2] = transform[2].map(|v| v * config.flatten);
            } else if interior.is_some() {
                // Indoors relief is cut walls: they block by their outline,
                // except where the source lets an actor stand.
                let mut walls = BTreeSet::new();
                interior::blocked_by_walls(
                    plane,
                    mesh,
                    &r.transform.matrix(),
                    ground_z,
                    config.step_height,
                    BLOCK_REACH,
                    &mut walls,
                );
                blocked.extend(walls.difference(&config.open_hexes));
            } else {
                overlay::blocked_under(
                    plane,
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
        let mut baked = overlay::bake(
            renderer,
            projection,
            anchor,
            &pieces,
            textures,
            config.render_tile,
        )?;
        if let Some((profile, _)) = decision {
            let tone = if flatten {
                &profile.tones.floor
            } else {
                &profile.tones.props
            };
            for o in &mut baked {
                tone.apply(&mut o.rgba);
                if fading {
                    for px in o.rgba.as_chunks_mut::<4>().0 {
                        px[3] = (f32::from(px[3]) * fade + 0.5) as u8;
                    }
                }
            }
        }
        overlays.extend(baked);
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
    if let Some(t) = &mut indoor {
        t.count_skips(&report.skips);
    }
    report.interior = indoor;
    if let Some((_, d)) = decision {
        let mut stats = d.stats.clone();
        cut_stats.walkers_covered_before = covered.0.len();
        cut_stats.walkers_covered_after = covered.1.len();
        cut_stats.door_walkers_before = door_covered.0.len();
        cut_stats.door_walkers_after = door_covered.1.len();
        stats.cutaway = cut_stats;
        stats.outlined = outlined;
        stats.outlined_kinds = outlined_kinds;
        if let Some((references, triangles)) = roof_stats {
            stats.roof_cut_references = references;
            stats.roof_cut_triangles = triangles;
        }
        report.declutter = Some(stats);
    }
    report.landmarks.unreachable.sort();
    if config.furniture.is_some() {
        // A footprint that would cut the open floor in two (a shelf's box
        // across a narrow passage) is not given: the reachable floor must
        // not fall apart.
        let mut free: BTreeSet<i32> = config
            .open_hexes
            .iter()
            .copied()
            .filter(|h| !blocked.contains(h))
            .collect();
        for p in &placements {
            if p.blocking {
                free.remove(&p.hex);
            }
        }
        let keys: Vec<String> = footprints.keys().cloned().collect();
        for reference in keys {
            let hexes = footprint_cover(plane, &footprints[&reference]);
            let taken: BTreeSet<i32> = hexes.intersection(&free).copied().collect();
            if taken.is_empty() {
                continue;
            }
            let before = components(&free);
            let after_free: BTreeSet<i32> = free.difference(&taken).copied().collect();
            if components(&after_free) > before {
                footprints.remove(&reference);
                if let Some(i) = prepared
                    .refs
                    .iter()
                    .position(|r| r.form.to_string() == reference)
                {
                    furniture_choice.insert(i, "keeps a passage open");
                }
            } else {
                free = after_free;
            }
        }
        let mut choices: Vec<crate::FurnitureChoice> = furniture_choice
            .iter()
            .map(|(&i, why)| {
                let r = prepared.refs[i];
                crate::FurnitureChoice {
                    reference: r.form.to_string(),
                    model: model_of(r),
                    reason: (*why).to_owned(),
                    footprint: footprints.contains_key(&r.form.to_string()),
                }
            })
            .collect();
        choices.sort();
        report.furniture = Some(crate::FurnitureStats {
            choices,
            footprints: footprints.len(),
            body_hexes: body_total.len(),
            open_blocked: open_blocked.len(),
        });
    }
    report.overlays = overlays.len();
    report.blocked_hexes = blocked.len();
    let contributors = store.contributors();
    report.set_contributors(contributors.clone(), config.top_contributors);

    placements.sort_by(|a, b| {
        a.hex
            .cmp(&b.hex)
            .then_with(|| a.reference.cmp(&b.reference))
            .then(a.art.cmp(&b.art))
    });
    skipped.sort();
    Ok(Run {
        sliced: Sliced {
            art: store.art,
            placements,
            overlays,
            blocked_hexes: blocked.into_iter().collect(),
            skipped,
            report,
            footprints,
        },
        contributors,
    })
}

/// The facts of reference `i`'s mesh, when it was read.
fn facts_of(prepared: &Prepared<'_>, i: usize) -> Option<MeshFacts> {
    let r = prepared.refs[i];
    match r.mesh.as_ref().map(|h| prepared.meshes.get(h)) {
        Some(Some(Ok((_, facts)))) => Some(*facts),
        _ => None,
    }
}

/// The normalised model path of a reference.
fn model_path(r: &Reference) -> String {
    r.model
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('/', "\\")
}

/// Whether a wire candidate's render loses its lattice to the alpha test:
/// rendered with its materials' cutoffs, it keeps at most `max_kept` of the
/// opaque pixels it has at the wire cutoff. Cached by render.
struct WireTest<'a> {
    renderer: &'a mut dyn Renderer,
    verdicts: BTreeMap<RenderKey, bool>,
}

impl WireTest<'_> {
    fn vanishes(
        &mut self,
        key: &RenderKey,
        mesh: &StaticMesh,
        textures: &dyn foundry_render::TextureSource,
        projection: &Projection,
        rule: &crate::WireRule,
        config: &SlicerConfig,
    ) -> bool {
        if let Some(&v) = self.verdicts.get(key) {
            return v;
        }
        let opaque = |renderer: &mut dyn Renderer, cutoff: Option<f32>| {
            render::render_canonical_with(
                renderer,
                projection,
                mesh,
                key.matrix(config),
                textures,
                config.render_tile,
                config.max_canvas_pixels,
                render::Look {
                    two_sided: false,
                    alpha_cutoff: cutoff,
                },
            )
            .ok()
            .flatten()
            .map_or(0, |c| {
                (0..c.width as usize * c.height as usize)
                    .filter(|&i| c.opaque(i))
                    .count()
            })
        };
        let wire = opaque(self.renderer, Some(rule.cutoff));
        let plain = opaque(self.renderer, None);
        let v = wire > 0 && (plain as f64) <= rule.max_kept * wire as f64;
        self.verdicts.insert(key.clone(), v);
        v
    }
}

/// The wire rule's look bits for reference `i` ([`crate::WireRule`]).
#[allow(clippy::too_many_arguments)]
fn look_of(
    prepared: &Prepared<'_>,
    i: usize,
    class: &Class,
    config: &SlicerConfig,
    textures: &dyn foundry_render::TextureSource,
    projection: &Projection,
    test: &mut WireTest<'_>,
) -> u8 {
    let Some(rule) = &config.wire else {
        return 0;
    };
    let r = prepared.refs[i];
    let model = model_path(r);
    let mut look = 0;
    let mesh = match r.mesh.as_ref().map(|h| prepared.meshes.get(h)) {
        Some(Some(Ok((mesh, _)))) => Some(mesh),
        _ => None,
    };
    if *class == Class::Sprite(Role::Scenery)
        && let (Some(mesh), Some(key)) = (mesh, RenderKey::of(r, config))
    {
        let candidate =
            crate::classify::masked_share(mesh).is_some_and(|share| share >= rule.min_masked_share);
        if config.matches(&rule.exclude_patterns, &model).is_none()
            && (config.matches(&rule.patterns, &model).is_some()
                || (candidate && test.vanishes(&key, mesh, textures, projection, rule, config)))
        {
            look |= LOOK_WIRE;
        }
    }
    if config.interior.is_none()
        && matches!(class, Class::Sprite(_) | Class::Sliced | Class::Landmark)
        && config.matches(&rule.two_sided_patterns, &model).is_some()
    {
        look |= LOOK_TWO_SIDED;
    }
    look
}

/// Per primitive and triangle of `mesh` placed by `matrix`, whether it is
/// roof over walkable ground ([`crate::RoofCut`]): every corner more than
/// `clearance` above the terrain, its ground outline within `reach` of a
/// walker's hex centre.
fn roof_mask(
    plane: &Plane,
    sampler: &GroundSampler,
    mesh: &StaticMesh,
    matrix: &[[f32; 4]; 3],
    walkers: &BTreeSet<i32>,
    rule: &crate::RoofCut,
) -> Vec<Vec<bool>> {
    let origin_ground = sampler
        .height(matrix[0][3], matrix[1][3])
        .unwrap_or(matrix[2][3]);
    mesh.primitives
        .iter()
        .map(|primitive| {
            if !primitive.is_well_formed() {
                return Vec::new();
            }
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
            primitive
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|triangle| {
                    let corners = triangle.map(|i| placed[i as usize]);
                    if corners.iter().flatten().any(|v| !v.is_finite()) {
                        return false;
                    }
                    let high = corners.iter().all(|p| {
                        let ground = sampler
                            .height(p[0] as f32, p[1] as f32)
                            .unwrap_or(origin_ground);
                        p[2] - f64::from(ground) > f64::from(rule.clearance)
                    });
                    if !high {
                        return false;
                    }
                    // Roofs, canopies and soffits face up or down and go
                    // within `reach`; a wall beside the walker is not over
                    // it, so steep triangles go only within `wall_reach`
                    // (a fascia or a post right over the hex).
                    let u = [0, 1, 2].map(|k| corners[1][k] - corners[0][k]);
                    let v = [0, 1, 2].map(|k| corners[2][k] - corners[0][k]);
                    let n = [
                        u[1] * v[2] - u[2] * v[1],
                        u[2] * v[0] - u[0] * v[2],
                        u[0] * v[1] - u[1] * v[0],
                    ];
                    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    let flat = length > 0.0 && (n[2] / length).abs() >= rule.min_tilt_cos;
                    let reach = if flat { rule.reach } else { rule.wall_reach };
                    let tile = corners.map(|p| plane.tile_of_source([p[0], p[1]]));
                    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
                    for p in &tile {
                        for k in 0..2 {
                            lo[k] = lo[k].min(p[k] - reach);
                            hi[k] = hi[k].max(p[k] + reach);
                        }
                    }
                    let around = [
                        [lo[0], lo[1]],
                        [hi[0], lo[1]],
                        [hi[0], hi[1]],
                        [lo[0], hi[1]],
                    ];
                    plane.hexes_in(&around).into_iter().any(|h| {
                        walkers.contains(&h)
                            && plane
                                .hex_tile(h)
                                .is_some_and(|c| distance_to_polygon(&tile, c) <= reach)
                    })
                })
                .collect()
        })
        .collect()
}

/// `mesh` without the triangles `mask` marks.
fn without_triangles(mesh: &StaticMesh, mask: &[Vec<bool>]) -> StaticMesh {
    let primitives = mesh
        .primitives
        .iter()
        .enumerate()
        .filter_map(|(p, primitive)| {
            let Some(m) = mask.get(p).filter(|m| !m.is_empty()) else {
                return Some(primitive.clone());
            };
            let indices: Vec<u32> = primitive
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .enumerate()
                .filter(|(t, _)| !m.get(*t).copied().unwrap_or(false))
                .flat_map(|(_, tri)| tri.iter().copied())
                .collect();
            (!indices.is_empty()).then(|| nif::Primitive {
                indices,
                ..primitive.clone()
            })
        })
        .collect();
    StaticMesh {
        primitives,
        collision: mesh.collision.clone(),
        skipped_blocks: mesh.skipped_blocks.clone(),
    }
}

/// Furniture reasons that are furniture (the others say why not).
const FURNITURE: [&str; 3] = ["scenery sprite", "container", "sliced"];

fn is_furniture(why: &str) -> bool {
    FURNITURE.contains(&why)
}

/// What the furniture rule makes of reference `i` (the `furniture`
/// module): one of [`FURNITURE`], a reason it is not, or `None` when it is
/// not considered at all (doors, items, ground art, landmarks).
fn furniture_of(
    prepared: &Prepared<'_>,
    i: usize,
    class: &Class,
    rule: &crate::FurnitureRule,
    config: &SlicerConfig,
    sampler: &GroundSampler,
    decision: Option<(&declutter::DeclutterProfile, &Decision)>,
) -> Option<&'static str> {
    let kind = match class {
        Class::Sprite(Role::Scenery) => "scenery sprite",
        Class::Sprite(Role::Container) => "container",
        Class::Sliced => "sliced",
        _ => return None,
    };
    let r = prepared.refs[i];
    let model = model_path(r);
    let facts = facts_of(prepared, i)?;
    let bounds = facts.bounds?;
    if !r.transform.is_finite() {
        return None;
    }
    let f = Footprint::of(&r.transform, bounds);
    let [x, y, z] = r.transform.position;
    let floor = match &config.interior {
        Some(indoors) => indoors.floor_z,
        None => sampler.height(x, y).unwrap_or(z),
    };
    let why = if rule.exclude_refs.contains(&r.form.to_string())
        || config.matches(&rule.exclude_patterns, &model).is_some()
    {
        "excluded"
    } else if config
        .door_kinds
        .iter()
        .any(|k| r.base_kind.as_deref() == Some(k.as_str()))
    {
        "door"
    } else if f.z[0] - floor > rule.max_lift {
        "raised"
    } else if kind == "scenery sprite" && config.matches(&config.sprite_patterns, &model).is_some()
    {
        "vegetation"
    } else if kind != "sliced" && f.width() <= 8.0 && f.longest_side() >= 20.0 {
        "wall plate"
    } else if (kind == "sliced" && decision.is_some_and(|(_, d)| d.cutaway[i]))
        || config
            .interior
            .as_ref()
            .is_some_and(|indoors| f.z[1] - indoors.floor_z >= indoors.cut_height - 16.0)
        || f.height() > rule.max_height
    {
        // A wall, or a post or corner piece of one (often a sprite): its
        // box would close the doorway beside it.
        "structural"
    } else if f.area > 0.0 && f.hexes(prepared.tile_units) > rule.max_hexes {
        "too large"
    } else {
        kind
    };
    Some(why)
}

/// The hexes a footprint (source corners) covers as the map builder takes
/// it: centre inside, or within 0.15 floor squares
/// (`map_builder::furniture::FOOTPRINT_REACH`).
fn footprint_cover(plane: &Plane, corners: &[[f32; 2]]) -> BTreeSet<i32> {
    const REACH: f64 = 0.15;
    let tile: Vec<[f64; 2]> = corners
        .iter()
        .map(|c| plane.tile_of_source([f64::from(c[0]), f64::from(c[1])]))
        .collect();
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in &tile {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k] - REACH);
            hi[k] = hi[k].max(p[k] + REACH);
        }
    }
    let around = [
        [lo[0], lo[1]],
        [hi[0], lo[1]],
        [hi[0], hi[1]],
        [lo[0], hi[1]],
    ];
    plane
        .hexes_in(&around)
        .into_iter()
        .filter(|&h| {
            plane
                .hex_tile(h)
                .is_some_and(|c| distance_to_polygon(&tile, c) <= REACH)
        })
        .collect()
}

/// Connected components (six neighbours) of a set of hexes.
fn components(hexes: &BTreeSet<i32>) -> usize {
    let mut seen: BTreeSet<i32> = BTreeSet::new();
    let mut count = 0;
    for &start in hexes {
        if !seen.insert(start) {
            continue;
        }
        count += 1;
        let mut stack = vec![start];
        while let Some(h) = stack.pop() {
            for n in hex::neighbours(h).into_iter().flatten() {
                if hexes.contains(&n) && seen.insert(n) {
                    stack.push(n);
                }
            }
        }
    }
    count
}

/// Reference `i`'s placed footprint polygon, source `x, y`, as the builder
/// takes it.
fn footprint_of(prepared: &Prepared<'_>, i: usize) -> Option<Vec<[f32; 2]>> {
    let r = prepared.refs[i];
    let bounds = facts_of(prepared, i)?.bounds?;
    let polygon = Footprint::of(&r.transform, bounds).polygon;
    (polygon.len() >= 3).then(|| polygon.iter().map(|p| [p[0] as f32, p[1] as f32]).collect())
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
    let mut prepared = prepare(assets, references, tile_units, config);
    prepared.declutter(&plane, projection, window, config);
    let mut report = BudgetReport::new(true, budget);
    let mut sprite_keys: BTreeSet<(ArtCategory, RenderKey)> = BTreeSet::new();
    let mut renders: BTreeMap<ClassKind, BTreeSet<RenderKey>> = BTreeMap::new();
    let mut contributions: BTreeMap<(ArtCategory, String, ClassKind), (usize, usize)> =
        BTreeMap::new();
    for (i, (r, class)) in prepared.refs.iter().zip(&prepared.classes).enumerate() {
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
            Class::Landmark => {
                let Some(key) = RenderKey::of(r, config) else {
                    continue;
                };
                renders.entry(kind).or_default().insert(key);
                let n = landmark_estimate(&prepared, i, projection, config);
                let c = contributions
                    .entry((ArtCategory::Scenery, model, kind))
                    .or_default();
                c.0 += 1;
                c.1 += n;
                report.class_mut(kind).entries += n;
                report.category_mut(ArtCategory::Scenery).used += n;
                report.landmarks.references += 1;
                report.landmarks.pieces += n;
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
                    estimate_strips(
                        &hexes,
                        config.merge_interior_runs,
                        config.max_perimeter_run,
                        config.interior.is_some(),
                    )
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
    report.interior = config.interior.as_ref().map(|settings| {
        let mut stats = InteriorStats::new(settings);
        stats.count_skips(&report.skips);
        stats
    });
    report.declutter = prepared.decision.map(|d| d.stats);
    Ok(report)
}

/// Strips a footprint would give: row runs of interior hexes count once
/// (when merged), row runs of perimeter hexes once per piece of at most
/// `max_run`. With `floor_baked` (interior mode) interior hexes are floor
/// and count nothing.
fn estimate_strips(
    hexes: &[i32],
    merge_interior: bool,
    max_run: usize,
    floor_baked: bool,
) -> usize {
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
            (true, _) if floor_baked => 0,
            (true, true) => 1,
            (true, false) => len,
            (false, _) => len.div_ceil(max_run.max(1)),
        };
        i = j + 1;
    }
    count
}
