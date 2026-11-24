//! What each reference becomes.

use std::fmt;

use mir::{Reference, Transform};
use nif::StaticMesh;

use crate::hex::{convex_hull, signed_area};
use crate::{ArtCategory, Role, SlicerConfig};

/// What the classifier needs to know about a mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshFacts {
    /// Minimum and maximum corner of the vertices, mesh-local; `None` for a
    /// mesh without usable geometry.
    pub bounds: Option<[[f32; 3]; 2]>,
    /// Total triangle area, mesh-local square units; `None` when unknown
    /// (bounds read from a manifest only).
    pub surface_area: Option<f64>,
    /// Triangles of the well-formed primitives.
    pub triangles: u64,
}

impl MeshFacts {
    /// Facts measured from a loaded mesh (well-formed primitives only).
    pub fn of(mesh: &StaticMesh) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut area = 0.0f64;
        let mut triangles = 0u64;
        for primitive in mesh.primitives.iter().filter(|p| p.is_well_formed()) {
            for p in &primitive.positions {
                if p.iter().all(|v| v.is_finite()) {
                    for axis in 0..3 {
                        min[axis] = min[axis].min(p[axis]);
                        max[axis] = max[axis].max(p[axis]);
                    }
                }
            }
            for tri in primitive.indices.as_chunks::<3>().0 {
                let [a, b, c] = tri.map(|i| primitive.positions[i as usize].map(f64::from));
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let n = [
                    u[1] * w[2] - u[2] * w[1],
                    u[2] * w[0] - u[0] * w[2],
                    u[0] * w[1] - u[1] * w[0],
                ];
                let twice = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if twice.is_finite() {
                    area += twice * 0.5;
                }
                triangles += 1;
            }
        }
        let bounds = (min.iter().all(|v| v.is_finite()) && triangles > 0).then_some([min, max]);
        MeshFacts {
            bounds,
            surface_area: Some(area),
            triangles,
        }
    }

    /// Facts from bounds alone, as `mir::MeshInfo` keeps them.
    pub fn from_bounds(bounds: Option<[[f32; 3]; 2]>, triangles: u64) -> Self {
        MeshFacts {
            bounds,
            surface_area: None,
            triangles,
        }
    }
}

/// A placed mesh's extent on the ground and in height, in source space.
#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    /// Convex hull of the transformed bounding box's corners, `(x, y)`,
    /// counter-clockwise.
    pub polygon: Vec<[f64; 2]>,
    /// Its area, square units.
    pub area: f64,
    /// Lowest and highest `z` of the transformed box.
    pub z: [f32; 2],
}

impl Footprint {
    /// The footprint of `bounds` placed by `transform`.
    pub fn of(transform: &Transform, bounds: [[f32; 3]; 2]) -> Self {
        let m = transform.matrix();
        let mut points = Vec::with_capacity(8);
        let mut z = [f32::INFINITY, f32::NEG_INFINITY];
        for corner in 0..8 {
            let local = [
                bounds[corner & 1][0],
                bounds[(corner >> 1) & 1][1],
                bounds[(corner >> 2) & 1][2],
            ];
            let p = [0, 1, 2]
                .map(|r| m[r][0] * local[0] + m[r][1] * local[1] + m[r][2] * local[2] + m[r][3]);
            points.push([f64::from(p[0]), f64::from(p[1])]);
            z = [z[0].min(p[2]), z[1].max(p[2])];
        }
        let polygon = convex_hull(&points);
        let area = if polygon.len() >= 3 {
            signed_area(&polygon).abs()
        } else {
            0.0
        };
        Footprint { polygon, area, z }
    }

    /// Height of the placed box.
    pub fn height(&self) -> f32 {
        (self.z[1] - self.z[0]).max(0.0)
    }

    /// Floor area in hexes, for floor squares of `tile_units`.
    pub fn hexes(&self, tile_units: f32) -> f64 {
        let hex_area = f64::from(tile_units) * f64::from(tile_units) / 4.0;
        self.area / hex_area
    }

    /// The longest side of the polygon, in units.
    pub fn longest_side(&self) -> f64 {
        let n = self.polygon.len();
        (0..n)
            .map(|i| {
                let a = self.polygon[i];
                let b = self.polygon[(i + 1) % n];
                ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
            })
            .fold(0.0, f64::max)
    }

    /// The narrowest width of the polygon, in units: the least distance
    /// between two parallel lines that hold it (0 for a degenerate one).
    pub fn width(&self) -> f64 {
        let n = self.polygon.len();
        if n < 3 {
            return 0.0;
        }
        (0..n)
            .map(|i| {
                let a = self.polygon[i];
                let b = self.polygon[(i + 1) % n];
                let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                if len <= 0.0 {
                    return f64::INFINITY;
                }
                self.polygon
                    .iter()
                    .map(|p| {
                        ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])).abs() / len
                    })
                    .fold(0.0, f64::max)
            })
            .fold(f64::INFINITY, f64::min)
    }
}

/// Why a reference produced nothing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
    /// Deleted by the winning plugin.
    Deleted,
    /// Starts disabled.
    Disabled,
    /// An invisible trigger or activator volume (`mir::Reference::
    /// trigger_volume`): its base's mesh is an editor stand-in.
    TriggerVolume,
    /// Its enable parent starts it disabled.
    DisabledByParent,
    /// The base kind is not static scenery (an actor, a sound).
    Kind(String),
    /// No model, or the model was not converted.
    NoMesh,
    /// The model path matches a skip pattern (named without a leading
    /// `*`).
    Pattern(String),
    /// The mesh could not be read.
    Unreadable(String),
    /// The mesh has no usable triangles.
    NoGeometry,
    /// Triangle area far below the footprint: wires, cables.
    Sparse,
    /// The render has no opaque pixel.
    EmptyRender,
    /// The object stands outside the map, or none of its pixels land on it.
    OutsideMap,
    /// The render would exceed `max_canvas_pixels`.
    TooLarge,
    /// Rendering, quantizing or framing failed.
    Failed(String),
    /// Interior mode: the placed bounds lie wholly above the ceiling cut
    /// (an upper storey, a roof), or nothing of the mesh is left under it.
    AboveCut,
    /// Interior mode: it reaches above the cut and hangs higher than
    /// `hang_clearance` (a lamp, a fan, a chandelier, a ceiling sign).
    Hanging,
    /// Interior mode: the placed bounds lie wholly below the floor band (a
    /// lower storey, a basement).
    BelowFloor,
    /// Declutter: micro-clutter, under every size threshold
    /// ([`crate::ClutterRules`]).
    Clutter,
    /// Declutter: the model matches a clutter pattern (named without a
    /// leading `*`).
    ClutterPattern(String),
    /// Declutter: one too many small props in a dense pile
    /// ([`crate::DensityRule`]).
    ClutterDensity,
    /// Declutter: a flat decal, dropped ([`crate::DecalMode::Drop`]).
    Decal,
    /// Declutter, interior mode: a beam, duct, plank or fixture hanging
    /// over walkable hexes ([`crate::OverhangRule`]).
    Overhang,
}

impl SkipReason {
    /// A short stable name, for counting.
    pub fn label(&self) -> &'static str {
        match self {
            SkipReason::Deleted => "deleted",
            SkipReason::Disabled => "initially disabled",
            SkipReason::TriggerVolume => "trigger volume",
            SkipReason::DisabledByParent => "disabled by enable parent",
            SkipReason::Kind(_) => "not scenery",
            SkipReason::NoMesh => "no mesh",
            SkipReason::Pattern(_) => "skip pattern",
            SkipReason::Unreadable(_) => "unreadable mesh",
            SkipReason::NoGeometry => "no geometry",
            SkipReason::Sparse => "sparse",
            SkipReason::EmptyRender => "empty render",
            SkipReason::OutsideMap => "outside the map",
            SkipReason::TooLarge => "too large to render",
            SkipReason::Failed(_) => "failed",
            SkipReason::AboveCut => "above the ceiling cut",
            SkipReason::Hanging => "hanging from the ceiling",
            SkipReason::BelowFloor => "below the floor",
            SkipReason::Clutter => "clutter",
            SkipReason::ClutterPattern(_) => "clutter pattern",
            SkipReason::ClutterDensity => "clutter density",
            SkipReason::Decal => "decal",
            SkipReason::Overhang => "overhang",
        }
    }

    /// Whether the declutter pass gave this reason.
    pub fn is_declutter(&self) -> bool {
        matches!(
            self,
            SkipReason::Clutter
                | SkipReason::ClutterPattern(_)
                | SkipReason::ClutterDensity
                | SkipReason::Decal
                | SkipReason::Overhang
        )
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Kind(kind) => write!(f, "base kind {kind} is not scenery"),
            SkipReason::Pattern(p) => write!(f, "model matches skip pattern {p:?}"),
            SkipReason::Unreadable(e) => write!(f, "mesh unreadable: {e}"),
            SkipReason::Failed(e) => write!(f, "failed: {e}"),
            SkipReason::ClutterPattern(p) => write!(f, "model matches clutter pattern {p:?}"),
            other => f.write_str(other.label()),
        }
    }
}

/// What a reference becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// Nothing.
    Skip(SkipReason),
    /// Flattened onto the ground and baked into the floor.
    GroundFlat,
    /// Rendered in relief, baked into the floor, its footprint blocked.
    GroundRelief,
    /// One art entry.
    Sprite(Role),
    /// Per-hex strips in the walls list.
    Sliced,
    /// Upright, but too large to slice: a few large pieces in the scenery
    /// list, each drawn whenever it shows and sorted against every walker
    /// as far as one sprite can be, its footprint blocked (crate
    /// documentation, "Landmarks").
    Landmark,
}

/// The class without its payload, for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClassKind {
    Skipped,
    GroundFlat,
    GroundRelief,
    Sprite,
    Sliced,
    Landmark,
}

impl Class {
    pub fn kind(&self) -> ClassKind {
        match self {
            Class::Skip(_) => ClassKind::Skipped,
            Class::GroundFlat => ClassKind::GroundFlat,
            Class::GroundRelief => ClassKind::GroundRelief,
            Class::Sprite(_) => ClassKind::Sprite,
            Class::Sliced => ClassKind::Sliced,
            Class::Landmark => ClassKind::Landmark,
        }
    }
}

impl ClassKind {
    pub const ALL: [ClassKind; 6] = [
        ClassKind::Skipped,
        ClassKind::GroundFlat,
        ClassKind::GroundRelief,
        ClassKind::Sprite,
        ClassKind::Sliced,
        ClassKind::Landmark,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ClassKind::Skipped => "skipped",
            ClassKind::GroundFlat => "ground flat",
            ClassKind::GroundRelief => "ground relief",
            ClassKind::Sprite => "sprite",
            ClassKind::Sliced => "sliced",
            ClassKind::Landmark => "landmark",
        }
    }
}

impl fmt::Display for ClassKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Role {
    /// The art list a role's art goes to.
    pub fn category(self) -> ArtCategory {
        match self {
            Role::WallStrip => ArtCategory::Walls,
            Role::Scenery | Role::Door | Role::FloorPiece => ArtCategory::Scenery,
            // Fallout 2 containers are item prototypes; their art is in the
            // items list.
            Role::Container | Role::Item => ArtCategory::Items,
        }
    }

    /// Whether a placed object of this role blocks movement.
    pub fn blocking(self) -> bool {
        !matches!(self, Role::Item | Role::FloorPiece)
    }
}

fn kind_in(list: &[String], kind: Option<&str>) -> bool {
    kind.is_some_and(|k| list.iter().any(|l| l == k))
}

/// Whether a low mesh named like a paved slab is shaped like one: wide
/// across its narrowest side, and (when the triangle area is known) with
/// about as much surface as floor, unlike a kerb or a kerb ring.
fn is_slab(footprint: &Footprint, facts: &MeshFacts, scale: f32, config: &SlicerConfig) -> bool {
    if footprint.width() < config.slab_min_width {
        return false;
    }
    match facts.surface_area {
        Some(area) => {
            let scale = f64::from(scale);
            area * scale * scale >= config.slab_min_fill * footprint.area
        }
        None => true,
    }
}

/// Classifies one reference from its record and its mesh's facts, for
/// floor squares of `tile_units` source units.
///
/// The rules, first match wins:
///
/// 1. deleted, initially disabled, a trigger volume → skip;
/// 2. a base kind in `skip_kinds` → skip;
/// 3. no model or no converted mesh → skip; a model matching
///    `skip_patterns` → skip;
/// 4. `facts` missing or without geometry → skip;
/// 5. items, doors and containers → one sprite each, whatever their size;
/// 6. a model matching `flat_patterns`, or a height under `flat_height` →
///    ground flat; a model matching `slab_patterns` under `slab_height`
///    that is slab-shaped (`slab_min_width`, `slab_min_fill`) → ground flat;
/// 7. a model matching `sprite_patterns` (vegetation) → a scenery sprite;
/// 8. a model matching `relief_patterns` → ground relief; a footprint
///    above `max_sliced_hexes` → a landmark, or ground relief when it
///    matches `terrain_patterns` or `landmarks` is off;
/// 9. a footprint of at most `sprite_max_hexes`, no side longer than
///    `sprite_max_span` → a scenery sprite;
/// 10. triangle area under `sparse_ratio` of the footprint area (wires) →
///     a scenery sprite, or skipped when `sparse_as_sprite` is off;
/// 11. otherwise → sliced.
///
/// With [`SlicerConfig::interior`] set (the `interior` module): after rule
/// 4, placed bounds wholly above the ceiling cut, hanging from it, or
/// wholly below the floor band → skip; rule 6 applies only to what lies
/// within the floor band; rule 8 becomes: a footprint above
/// `max_sliced_hexes` → sliced, relief and terrain patterns ignored.
///
/// Rendering can still skip a sprite or a sliced reference later (an empty
/// render, outside the map).
pub fn classify(
    reference: &Reference,
    facts: Option<&MeshFacts>,
    tile_units: f32,
    config: &SlicerConfig,
) -> Class {
    if reference.flags.deleted {
        return Class::Skip(SkipReason::Deleted);
    }
    if reference.flags.initially_disabled {
        return Class::Skip(SkipReason::Disabled);
    }
    if reference.trigger_volume {
        return Class::Skip(SkipReason::TriggerVolume);
    }
    let kind = reference.base_kind.as_deref();
    if kind_in(&config.skip_kinds, kind) {
        return Class::Skip(SkipReason::Kind(kind.unwrap_or_default().to_owned()));
    }
    let (Some(model), Some(_)) = (reference.model.as_deref(), reference.mesh.as_deref()) else {
        return Class::Skip(SkipReason::NoMesh);
    };
    let model = model.to_ascii_lowercase().replace('/', "\\");
    if let Some(pattern) = config.matches(&config.skip_patterns, &model) {
        // Named as in the first pass: `*marker` reads `marker`.
        let name = pattern.strip_prefix('*').unwrap_or(&pattern).to_owned();
        return Class::Skip(SkipReason::Pattern(name));
    }
    let Some(facts) = facts else {
        return Class::Skip(SkipReason::NoMesh);
    };
    let Some(bounds) = facts.bounds else {
        return Class::Skip(SkipReason::NoGeometry);
    };
    if !reference.transform.is_finite() {
        return Class::Skip(SkipReason::NoGeometry);
    }
    let footprint = Footprint::of(&reference.transform, bounds);
    let interior = config.interior.as_ref();
    if let Some(reason) = interior.and_then(|i| i.skip(footprint.z)) {
        return Class::Skip(reason);
    }
    if kind_in(&config.item_kinds, kind) {
        return Class::Sprite(Role::Item);
    }
    if kind_in(&config.door_kinds, kind) {
        return Class::Sprite(Role::Door);
    }
    if kind_in(&config.container_kinds, kind) {
        return Class::Sprite(Role::Container);
    }
    // Indoors, only what lies on the floor is flattened onto it.
    let on_floor = interior.is_none_or(|i| i.on_floor(footprint.z[0]));
    if on_floor
        && (config.matches(&config.flat_patterns, &model).is_some()
            || footprint.height() < config.flat_height)
    {
        return Class::GroundFlat;
    }
    if on_floor
        && footprint.height() < config.slab_height
        && config.matches(&config.slab_patterns, &model).is_some()
        && is_slab(&footprint, facts, reference.transform.scale, config)
    {
        return Class::GroundFlat;
    }
    if config.matches(&config.sprite_patterns, &model).is_some() {
        return Class::Sprite(Role::Scenery);
    }
    let hexes = footprint.hexes(tile_units);
    if interior.is_some() {
        // Indoors there is no relief: rock is a wall, and a shell too
        // large to slice is still walls once its floor is baked and its
        // ceiling cut.
        if hexes > config.max_sliced_hexes {
            return Class::Sliced;
        }
    } else if config.matches(&config.relief_patterns, &model).is_some() {
        return Class::GroundRelief;
    }
    if hexes > config.max_sliced_hexes {
        return if config.landmarks && config.matches(&config.terrain_patterns, &model).is_none() {
            Class::Landmark
        } else {
            Class::GroundRelief
        };
    }
    if hexes <= config.sprite_max_hexes && footprint.longest_side() <= config.sprite_max_span {
        return Class::Sprite(Role::Scenery);
    }
    if let Some(area) = facts.surface_area {
        let scale = f64::from(reference.transform.scale);
        if config.sparse_ratio > 0.0
            && footprint.area > 0.0
            && area * scale * scale < config.sparse_ratio * footprint.area
        {
            return if config.sparse_as_sprite {
                Class::Sprite(Role::Scenery)
            } else {
                Class::Skip(SkipReason::Sparse)
            };
        }
    }
    Class::Sliced
}
