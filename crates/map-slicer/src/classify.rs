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
}

/// Why a reference produced nothing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
    /// Deleted by the winning plugin.
    Deleted,
    /// Starts disabled.
    Disabled,
    /// Its enable parent starts it disabled.
    DisabledByParent,
    /// The base kind is not static scenery (an actor, a sound).
    Kind(String),
    /// No model, or the model was not converted.
    NoMesh,
    /// The model path matches a skip pattern.
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
}

impl SkipReason {
    /// A short stable name, for counting.
    pub fn label(&self) -> &'static str {
        match self {
            SkipReason::Deleted => "deleted",
            SkipReason::Disabled => "initially disabled",
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
        }
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Kind(kind) => write!(f, "base kind {kind} is not scenery"),
            SkipReason::Pattern(p) => write!(f, "model matches skip pattern {p:?}"),
            SkipReason::Unreadable(e) => write!(f, "mesh unreadable: {e}"),
            SkipReason::Failed(e) => write!(f, "failed: {e}"),
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
}

/// The class without its payload, for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClassKind {
    Skipped,
    GroundFlat,
    GroundRelief,
    Sprite,
    Sliced,
}

impl Class {
    pub fn kind(&self) -> ClassKind {
        match self {
            Class::Skip(_) => ClassKind::Skipped,
            Class::GroundFlat => ClassKind::GroundFlat,
            Class::GroundRelief => ClassKind::GroundRelief,
            Class::Sprite(_) => ClassKind::Sprite,
            Class::Sliced => ClassKind::Sliced,
        }
    }
}

impl ClassKind {
    pub const ALL: [ClassKind; 5] = [
        ClassKind::Skipped,
        ClassKind::GroundFlat,
        ClassKind::GroundRelief,
        ClassKind::Sprite,
        ClassKind::Sliced,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ClassKind::Skipped => "skipped",
            ClassKind::GroundFlat => "ground flat",
            ClassKind::GroundRelief => "ground relief",
            ClassKind::Sprite => "sprite",
            ClassKind::Sliced => "sliced",
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

/// Classifies one reference from its record and its mesh's facts, for
/// floor squares of `tile_units` source units.
///
/// The rules, first match wins:
///
/// 1. deleted, initially disabled → skip;
/// 2. a base kind in `skip_kinds` → skip;
/// 3. no model or no converted mesh → skip; a model matching
///    `skip_patterns` → skip;
/// 4. `facts` missing or without geometry → skip;
/// 5. items, doors and containers → one sprite each, whatever their size;
/// 6. a model matching `flat_patterns`, or a height under `flat_height` →
///    ground flat;
/// 7. a model matching `sprite_patterns` (vegetation) → a scenery sprite;
/// 8. a model matching `relief_patterns`, or a footprint above
///    `max_sliced_hexes` → ground relief;
/// 9. a footprint of at most `sprite_max_hexes`, no side longer than
///    `sprite_max_span` → a scenery sprite;
/// 10. triangle area under `sparse_ratio` of the footprint area (wires) →
///     a scenery sprite, or skipped when `sparse_as_sprite` is off;
/// 11. otherwise → sliced.
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
    let kind = reference.base_kind.as_deref();
    if kind_in(&config.skip_kinds, kind) {
        return Class::Skip(SkipReason::Kind(kind.unwrap_or_default().to_owned()));
    }
    let (Some(model), Some(_)) = (reference.model.as_deref(), reference.mesh.as_deref()) else {
        return Class::Skip(SkipReason::NoMesh);
    };
    let model = model.to_ascii_lowercase().replace('/', "\\");
    if let Some(pattern) = SlicerConfig::matches(&config.skip_patterns, &model) {
        return Class::Skip(SkipReason::Pattern(pattern));
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
    if kind_in(&config.item_kinds, kind) {
        return Class::Sprite(Role::Item);
    }
    if kind_in(&config.door_kinds, kind) {
        return Class::Sprite(Role::Door);
    }
    if kind_in(&config.container_kinds, kind) {
        return Class::Sprite(Role::Container);
    }
    let footprint = Footprint::of(&reference.transform, bounds);
    if SlicerConfig::matches(&config.flat_patterns, &model).is_some()
        || footprint.height() < config.flat_height
    {
        return Class::GroundFlat;
    }
    if SlicerConfig::matches(&config.sprite_patterns, &model).is_some() {
        return Class::Sprite(Role::Scenery);
    }
    let hexes = footprint.hexes(tile_units);
    if SlicerConfig::matches(&config.relief_patterns, &model).is_some()
        || hexes > config.max_sliced_hexes
    {
        return Class::GroundRelief;
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
