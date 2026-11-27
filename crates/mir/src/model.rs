//! The documents of a MIR directory: cells, the manifest and their parts.
//!
//! Everything here is plain data with a stable JSON form. Positions and
//! rotations are in source space (`docs/contracts.md`, section 1): X east,
//! Y north, Z up, game units, radians.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::paths::FormKey;

/// Version of the directory layout, the JSON documents and the SQLite
/// schema. A directory written by another version is rebuilt by `extract`
/// and refused by [`crate::Mir::open`].
///
/// 2: actor documents (`actors/`), and `creatures.model` read against the
/// skeleton's folder.
///
/// 3: a reference's primitive volume (`Reference::primitive`, from `XPRM`)
/// is kept in the cell documents; a directory written before has none, so
/// no trigger volume has a shape or bounds.
pub const SCHEMA_VERSION: u32 = 3;

/// Version of the tables in `records.sqlite`, within one
/// [`SCHEMA_VERSION`]. Additions that leave every JSON document as it was
/// raise this number instead: `extract` rebuilds a `records.sqlite` whose
/// manifest records an older revision, and the reader methods of a table
/// an older file lacks return [`crate::MirError::MissingTable`].
///
/// 1: the game (`meta.game`), terminals, notes, packages, actor packages,
/// script and quest-target references, and the weapon combat columns.
/// A manifest without the field is revision 0.
///
/// 2: the simulation tables (`docs/simulation-layer-spec.md`, appendix A):
/// `globals`, `game_settings`, `messages`, `message_buttons`, `factions`,
/// `faction_relations`, `actor_ai` and `actor_factions` (resolved through
/// templates), `info_links`, `package_events`, `script_contexts`,
/// `script_ref_list`, `script_locals` and `decode_skips`; new columns on
/// `infos` (`flag_bytes`, `knam`, `knam_sig`, `speech_challenge`),
/// `packages` (`pkdt_size`, `behaviour_flags`, `behaviour_raw`,
/// `type_flags`) and `conditions` (`param1_ref`, `param2_ref`); and the
/// reasons `ConditionParam` and `TeleportDestination` of
/// `Records::ref_referrals`. No JSON document changed.
pub const RECORDS_REVISION: u32 = 2;

/// The game a directory written before [`Manifest::game`] existed came
/// from: every such directory was extracted from New Vegas.
pub(crate) fn legacy_game() -> fnv_esm::GameVariant {
    fnv_esm::GameVariant::FalloutNewVegas
}

/// Side of an exterior cell, in game units.
pub const CELL_UNITS: f32 = 4096.0;

/// Vertices along one side of a cell's land grid.
pub const LAND_GRID: usize = 33;

/// Distance between neighbouring land vertices, in game units.
pub const LAND_SPACING: f32 = 128.0;

/// The exterior cell that contains a source-space position.
///
/// ```
/// assert_eq!(mir::cell_of(&[-1.0, 4096.0, 0.0]), (-1, 1));
/// ```
pub fn cell_of(position: &[f32; 3]) -> (i32, i32) {
    let axis = |v: f32| (v / CELL_UNITS).floor() as i32;
    (axis(position[0]), axis(position[1]))
}

/// The part of a worldspace an extract covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slice {
    /// A name for the slice; it is recorded, nothing depends on it.
    pub name: String,
    /// Editor id of the worldspace; case does not matter.
    pub worldspace: String,
    /// Cell X range.
    pub x: RangeInclusive<i32>,
    /// Cell Y range.
    pub y: RangeInclusive<i32>,
    /// How many teleport-door hops into interiors are followed. 0 extracts
    /// no interior.
    pub interior_depth: u32,
}

impl Slice {
    /// Whether the exterior cell `(x, y)` is inside the slice.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.x.contains(&x) && self.y.contains(&y)
    }
}

/// How much of the lossless raw store `records.sqlite` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawStore {
    /// The subrecords of every record.
    #[default]
    All,
    /// The subrecords of the record kinds that have a typed table. Other
    /// kinds keep only their row in `records`.
    TypedOnly,
    /// No raw subrecords.
    None,
}

impl RawStore {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::TypedOnly => "typed_only",
            Self::None => "none",
        }
    }
}

/// Position, rotation and scale of a placed reference, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Position `(x, y, z)` in game units.
    pub position: [f32; 3],
    /// Euler angles `(rx, ry, rz)` in radians, positive clockwise looking
    /// down the axis.
    pub rotation: [f32; 3],
    /// Uniform scale; 1 when the reference stores none.
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: 1.0,
        }
    }
}

impl Transform {
    /// The 3 × 4 matrix that takes a mesh-local point to source space:
    /// rows of `[R·scale | position]` with
    /// `R = Rz(-rz) · Ry(-ry) · Rx(-rx)`, where `Rz(a)`, `Ry(a)` and
    /// `Rx(a)` are the usual counter-clockwise rotations of a right-handed
    /// frame.
    ///
    /// This is the only implementation of the rule of `docs/contracts.md`,
    /// section 1. The sign is settled for rotation about Z; the order of
    /// the three axes is provisional there.
    ///
    /// ```
    /// use std::f32::consts::FRAC_PI_2;
    /// let quarter = mir::Transform { position: [0.0; 3], rotation: [0.0, 0.0, FRAC_PI_2], scale: 1.0 };
    /// // A positive turn about Z is clockwise seen from above: east goes south.
    /// let south = quarter.apply([1.0, 0.0, 0.0]);
    /// assert!((south[0]).abs() < 1e-6 && (south[1] + 1.0).abs() < 1e-6);
    /// ```
    pub fn matrix(&self) -> [[f32; 4]; 3] {
        // Computed in f64 so that the composition adds no error of its own.
        let [rx, ry, rz] = self.rotation.map(|a| -f64::from(a));
        let (sx, cx) = rx.sin_cos();
        let (sy, cy) = ry.sin_cos();
        let (sz, cz) = rz.sin_cos();
        // Rz · Ry · Rx, written out.
        let r = [
            [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
            [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
            [-sy, cy * sx, cy * cx],
        ];
        let scale = f64::from(self.scale);
        let mut out = [[0.0f32; 4]; 3];
        for (row, (r_row, &translation)) in r.iter().zip(&self.position).enumerate() {
            for (column, &value) in r_row.iter().enumerate() {
                out[row][column] = (value * scale) as f32;
            }
            out[row][3] = translation;
        }
        out
    }

    /// Transforms a mesh-local point to source space with
    /// [`Transform::matrix`].
    pub fn apply(&self, point: [f32; 3]) -> [f32; 3] {
        let m = self.matrix();
        [0, 1, 2].map(|row| {
            m[row][0] * point[0] + m[row][1] * point[1] + m[row][2] * point[2] + m[row][3]
        })
    }

    /// Whether every component is finite.
    pub fn is_finite(&self) -> bool {
        self.position.iter().all(|v| v.is_finite())
            && self.rotation.iter().all(|v| v.is_finite())
            && self.scale.is_finite()
    }
}

/// Names an exterior cell of the directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CellKey {
    /// Editor id of the worldspace, lowercase.
    pub worldspace: String,
    /// Cell X.
    pub x: i32,
    /// Cell Y.
    pub y: i32,
}

/// Where a cell is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CellLocation {
    /// An exterior cell: it covers `x ∈ [4096·x, 4096·(x+1))` and likewise
    /// in Y.
    Exterior {
        /// Editor id of the worldspace, lowercase.
        worldspace: String,
        /// The worldspace record.
        worldspace_form: FormKey,
        /// Cell X.
        x: i32,
        /// Cell Y.
        y: i32,
    },
    /// An interior cell. Its coordinates are its own.
    Interior,
}

/// One cell: `cells/<worldspace>/<x>_<y>.json` or
/// `interiors/<editor_id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellDoc {
    /// [`SCHEMA_VERSION`] of the writer.
    pub schema_version: u32,
    /// The cell record.
    pub form: FormKey,
    /// The cell's editor id, as stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    /// The cell's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Exterior grid position, or interior.
    pub location: CellLocation,
    /// Cell flags, as stored.
    pub flags: u8,
    /// Water height in game units, when the cell stores one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_height: Option<f32>,
    /// The terrain; exterior cells only, and only when the cell has a
    /// height grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land: Option<LandDoc>,
    /// Placed references, sorted by form. For an exterior cell this
    /// includes the worldspace's persistent references that stand in it.
    pub references: Vec<Reference>,
    /// Navigation meshes, sorted by form.
    pub navmeshes: Vec<NavmeshDoc>,
}

impl CellDoc {
    /// Whether this is an interior cell.
    pub fn is_interior(&self) -> bool {
        self.location == CellLocation::Interior
    }

    /// The source-space position of the cell's south-west corner; the
    /// origin for an interior.
    pub fn origin(&self) -> [f32; 2] {
        match &self.location {
            CellLocation::Exterior { x, y, .. } => [*x as f32 * CELL_UNITS, *y as f32 * CELL_UNITS],
            CellLocation::Interior => [0.0, 0.0],
        }
    }

    /// The reference with the given form, if the cell has it.
    pub fn reference(&self, form: &FormKey) -> Option<&Reference> {
        self.references
            .binary_search_by(|r| r.form.cmp(form))
            .ok()
            .map(|index| &self.references[index])
    }
}

/// The terrain of an exterior cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LandDoc {
    /// The land record.
    pub form: FormKey,
    /// Absolute heights in game units: 33 × 33, row-major, row 0 at the
    /// south edge, column 0 at the west edge, 128 units apart.
    pub heights: Vec<f32>,
    /// Vertex normals, in the order of `heights`: `(x, y, z)` signed bytes,
    /// 127 is 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normals: Option<Vec<[i8; 3]>>,
    /// Vertex colours `(r, g, b)`, in the order of `heights`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<[u8; 3]>>,
    /// Texture layers, sorted by quadrant, then layer: each quadrant's base
    /// layer first, then its blended layers in blending order.
    pub layers: Vec<LandLayer>,
}

impl LandDoc {
    /// The height at a grid vertex; `None` outside the 33 × 33 grid.
    pub fn height(&self, row: usize, column: usize) -> Option<f32> {
        if row >= LAND_GRID || column >= LAND_GRID {
            return None;
        }
        self.heights.get(row * LAND_GRID + column).copied()
    }
}

/// One texture layer of one quadrant of a cell's terrain.
///
/// Quadrants are 17 × 17 vertices and share their inner edges: 0 is
/// south-west, 1 south-east, 2 north-west, 3 north-east.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LandLayer {
    /// The quadrant, 0 to 3.
    pub quadrant: u8,
    /// −1 for the quadrant's base layer, which covers it whole; otherwise
    /// the blended layer's number. Layers blend in ascending order.
    pub layer: i32,
    /// The land texture record; `None` is the game's default land texture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land_texture: Option<FormKey>,
    /// The land texture's editor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land_texture_editor_id: Option<String>,
    /// The diffuse texture path, normalised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// Content hash of that texture: `textures/<hash>.png`. `None` when the
    /// texture is missing or undecodable; the manifest says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture_hash: Option<String>,
    /// Opacity of a blended layer at the quadrant's vertices. A vertex not
    /// listed is fully transparent. Empty for a base layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opacity: Vec<LayerOpacity>,
    /// Set when the record gives this layer no usable land texture and the
    /// engine's default land texture stands in: `texture` is then that
    /// default ([`crate::Manifest::default_land_texture`]). `None` for a
    /// layer that names its own texture, and in every document written
    /// before this field existed (such documents also lack the base layer
    /// of a quadrant that stores none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaulted: Option<LandDefault>,
}

impl LandLayer {
    /// Whether the engine's default land texture stands in for this layer.
    pub fn is_defaulted(&self) -> bool {
        self.defaulted.is_some()
    }
}

/// Why a land layer is drawn with the engine's default land texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandDefault {
    /// The quadrant stores no base layer (`BTXT`); this base layer stands
    /// for the one the engine draws.
    NoBaseLayer,
    /// The layer's land texture is null.
    NullTexture,
    /// The layer names a form that is not a land texture of the plugin
    /// list.
    UnknownTexture,
    /// The land texture names no texture set, or one without a diffuse
    /// texture.
    NoTextureSet,
}

/// Opacity of a blended layer at one vertex of its quadrant.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerOpacity {
    /// `row * 17 + column` in the quadrant's grid, row 0 at its south edge
    /// and column 0 at its west edge.
    pub position: u16,
    /// 0 to 1.
    pub opacity: f32,
}

/// Which record type placed a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// A placed object.
    Object,
    /// A placed NPC.
    Npc,
    /// A placed creature.
    Creature,
}

/// Where in the plugin a reference was filed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefGroup {
    /// The cell's temporary children.
    Temporary,
    /// The cell's own persistent children.
    Persistent,
    /// The cell's visible-when-distant children.
    VisibleDistant,
    /// The worldspace's persistent cell: the reference was assigned to this
    /// cell by its position.
    WorldPersistent,
}

/// Record flags of a placed reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RefFlags {
    /// Always loaded, and addressable by scripts.
    pub persistent: bool,
    /// Starts disabled.
    pub initially_disabled: bool,
    /// Deleted by the plugin that wins the record.
    pub deleted: bool,
}

/// Identifies a cell from outside, for a teleport destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRef {
    /// The cell record. For a door filed in a worldspace's persistent cell
    /// this is that persistent cell; `location` then gives the grid cell
    /// the door stands in.
    pub form: FormKey,
    /// The cell's editor id, as stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    /// Exterior grid position, or interior.
    pub location: CellLocation,
}

/// Where a teleport door leads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeleportDoc {
    /// The destination door reference.
    pub door: FormKey,
    /// The cell the destination door is in; `None` when the door is not in
    /// the plugin list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<CellRef>,
    /// Where the traveller arrives.
    pub position: [f32; 3],
    /// The traveller's rotation on arrival, radians.
    pub rotation: [f32; 3],
}

/// The reference whose enabled state this one follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnableParentDoc {
    /// The parent reference.
    pub reference: FormKey,
    /// Enabled when the parent is disabled, and the other way round.
    pub opposite: bool,
}

/// The lock on a door or container reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockDoc {
    /// The lock level, as stored.
    pub level: u8,
    /// The key that opens it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<FormKey>,
    /// Lock flags, as stored.
    pub flags: u8,
}

/// A placed reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// The reference record.
    pub form: FormKey,
    /// The reference's editor id, as stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    /// Object, NPC or creature.
    pub kind: RefKind,
    /// Where the reference was filed.
    pub group: RefGroup,
    /// The base form being placed.
    pub base: FormKey,
    /// Record signature of the base form (`"STAT"`, `"DOOR"`, `"NPC_"`,
    /// ...); `None` when the base is not in the plugin list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_kind: Option<String>,
    /// The base form's editor id, as stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_editor_id: Option<String>,
    /// The base form's model path, normalised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Content hash of that model: `meshes/<hash>.glb`. `None` when the
    /// model is missing or undecodable; the manifest says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    /// Position, rotation and scale.
    pub transform: Transform,
    /// The record flags that matter to a map.
    pub flags: RefFlags,
    /// The door's destination, on a teleport door.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teleport: Option<TeleportDoc>,
    /// The enable parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_parent: Option<EnableParentDoc>,
    /// The lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<LockDoc>,
    /// The item count of a stacked item reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i32>,
    /// The linked reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_ref: Option<FormKey>,
    /// The owner (an NPC or a faction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<FormKey>,
    /// The reference is an invisible trigger or activator volume: a
    /// primitive (`XPRM`) on a reference to an activator (`ACTI`, `TACT`).
    /// Its base's model, if any, is an editor stand-in and is not drawn;
    /// the same base placed without a primitive is a visible activator.
    /// Primitives on markers, acoustic spaces and sounds do not set it.
    /// Absent from the JSON when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trigger_volume: bool,
    /// The reference's primitive volume (`XPRM`), whatever its base is: a
    /// trigger (see `trigger_volume`), but also the room, collision and
    /// multibound markers, acoustic spaces and portals. `None` when the
    /// reference has none or its bounds are not finite numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primitive: Option<PrimitiveDoc>,
}

/// The shape of a primitive volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveShape {
    /// A box (`1`).
    Box,
    /// A sphere (`2`).
    Sphere,
    /// A portal box (`3`): a box between rooms, which may be flat.
    PortalBox,
    /// A shape code no layout is known for.
    Other(u32),
}

impl PrimitiveShape {
    /// The shape a stored code names.
    pub fn from_code(code: u32) -> Self {
        match code {
            1 => PrimitiveShape::Box,
            2 => PrimitiveShape::Sphere,
            3 => PrimitiveShape::PortalBox,
            other => PrimitiveShape::Other(other),
        }
    }
}

/// A primitive volume (`XPRM`) as the MIR keeps it: the extents and the
/// shape. The editor colour and the unnamed float of the record are not
/// kept.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PrimitiveDoc {
    /// Extent along the reference's local X, Y and Z, as stored, in game
    /// units and before the reference's scale. Public documentation reads
    /// them as half-extents (a box reaches from `-bounds` to `+bounds`,
    /// and a sphere's first is its radius); that reading is not verified.
    pub bounds: [f32; 3],
    /// The shape.
    pub shape: PrimitiveShape,
}

/// Serde helper: skip a `false` flag.
fn is_false(value: &bool) -> bool {
    !*value
}

/// One triangle of a navigation mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavTriangleDoc {
    /// Indices into [`NavmeshDoc::vertices`]; always in range.
    pub vertices: [u32; 3],
    /// The link across each edge, as stored: the neighbouring triangle, or
    /// −1 for none. Edge `i` runs from vertex `i` to vertex `(i + 1) % 3`.
    /// Not range-checked: on an edge that leaves the mesh the value is an
    /// index into [`NavmeshDoc::external_links`] instead.
    pub links: [i32; 3],
    /// Triangle flags, as stored.
    pub flags: u32,
}

/// A door a navigation mesh leads to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavDoorLinkDoc {
    /// The door reference.
    pub door: FormKey,
    /// The triangle in front of the door.
    pub triangle: u32,
}

/// A link from a navigation mesh to another one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavExternalLinkDoc {
    /// The other navigation mesh.
    pub navmesh: FormKey,
    /// The triangle in it.
    pub triangle: u32,
}

/// A navigation mesh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavmeshDoc {
    /// The navmesh record.
    pub form: FormKey,
    /// Vertices `(x, y, z)` in game units; world space in an exterior cell.
    pub vertices: Vec<[f32; 3]>,
    /// Triangles.
    pub triangles: Vec<NavTriangleDoc>,
    /// Indices of the triangles that offer cover.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cover_triangles: Vec<u32>,
    /// Doors the mesh leads to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub door_links: Vec<NavDoorLinkDoc>,
    /// Links to other navigation meshes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_links: Vec<NavExternalLinkDoc>,
}

impl NavmeshDoc {
    /// The three corners of a triangle.
    pub fn corners(&self, triangle: usize) -> Option<[[f32; 3]; 3]> {
        let t = self.triangles.get(triangle)?;
        let corner = |i: u32| self.vertices.get(i as usize).copied();
        Some([
            corner(t.vertices[0])?,
            corner(t.vertices[1])?,
            corner(t.vertices[2])?,
        ])
    }
}

/// A decoded texture: RGBA8, row-major, top row first, straight alpha,
/// `width * height * 4` bytes (`docs/contracts.md`, section 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The pixels.
    pub rgba: Vec<u8>,
}

/// One plugin an extract read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginEntry {
    /// File name, lowercase.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 of the file, lowercase hex.
    pub sha256: String,
}

/// One archive of the lookup order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    /// File name, lowercase.
    pub name: String,
    /// Why the archive could not be opened, when a lookup needed it and it
    /// did not open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What became of one source path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetStatus {
    /// Converted: the file named by the hash exists.
    Converted,
    /// No loose file and no archive has the path.
    Missing,
    /// The file was found but could not be read from its archive or from
    /// disk.
    Unreadable,
    /// The file was read but could not be converted.
    Undecodable,
}

/// The manifest's entry for one source path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetEntry {
    /// What became of the path.
    pub status: AssetStatus,
    /// Content hash of the source bytes; present whenever they were read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The reason, for anything but [`AssetStatus::Converted`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AssetEntry {
    /// The hash, when the converted file exists.
    pub fn converted_hash(&self) -> Option<&str> {
        match self.status {
            AssetStatus::Converted => self.hash.as_deref(),
            _ => None,
        }
    }
}

/// What the manifest keeps about one converted mesh, so that reports and
/// later runs need not open the `.glb`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshInfo {
    /// Number of primitives (source shapes).
    pub primitives: u32,
    /// Total vertices.
    pub vertices: u32,
    /// Total triangles.
    pub triangles: u32,
    /// Minimum and maximum corner of the vertices, in mesh-local source
    /// space; `None` for a mesh without geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<[[f32; 3]; 2]>,
    /// Texture paths the mesh uses, normalised, sorted, without repeats.
    pub textures: Vec<String>,
    /// Block types the mesh parser skipped by size, with their counts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skipped_blocks: BTreeMap<String, u32>,
    /// Whether the mesh carries collision metadata.
    pub collision: bool,
    /// Primitives that are skinned (bound to a skeleton).
    #[serde(default)]
    pub skinned_primitives: u32,
}

/// What the manifest keeps about one converted texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// What the manifest keeps about one converted skeleton
/// (`skeletons/<hash>.json`, a `nif::Skeleton`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkeletonInfo {
    /// The source paths whose bytes have this hash, normalised, sorted.
    pub paths: Vec<String>,
    /// Number of bones.
    pub bones: u32,
    /// Bones on the longest root-to-leaf path.
    pub depth: u32,
    /// Bones without a parent.
    pub roots: u32,
}

/// What the manifest keeps about one converted animation
/// (`animations/<hash>.json`, a `nif::Animation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationInfo {
    /// The source paths whose bytes have this hash, normalised, sorted.
    pub paths: Vec<String>,
    /// The sequence name.
    pub name: String,
    /// Length in seconds.
    pub duration: f32,
    /// Whether the sequence repeats.
    pub looping: bool,
    /// Number of bone tracks.
    pub tracks: u32,
    /// Number of text keys.
    pub text_keys: u32,
    /// The accumulation root's bone name; empty when there is none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub accum_root: String,
}

/// One interior cell of the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteriorEntry {
    /// The file stem under `interiors/`: the editor id, lowercase.
    pub id: String,
    /// The cell record.
    pub form: FormKey,
    /// Teleport-door hops from the slice's exterior cells, from 1.
    pub depth: u32,
}

/// Whether an actor document describes an NPC or a creature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// An NPC (`NPC_`): a human built from race, outfit, head and hair.
    Npc,
    /// A creature (`CREA`): a skeleton and its own body models.
    Creature,
}

/// What a part of an actor's body is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartRole {
    /// A body part of the race (upper body, hands), or a creature's model.
    Body,
    /// A worn armour or armour addon.
    Outfit,
    /// The race's head.
    Head,
    /// The hair.
    Hair,
}

/// One mesh of an actor's body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorPart {
    /// What the part is.
    pub role: PartRole,
    /// The model path, normalised.
    pub path: String,
    /// Content hash of the model: `meshes/<hash>.glb`. `None` when the
    /// model is missing or undecodable; the manifest says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    /// The diffuse texture to use instead of the mesh's own: the race's
    /// skin texture for a body part or the head, normalised. `None` keeps
    /// the mesh's textures.
    pub texture: Option<String>,
    /// The record the part comes from: a race, an armour, an addon, a hair
    /// or a creature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<FormKey>,
}

/// Colours applied to an actor's parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActorTint {
    /// The hair colour, red, green, blue.
    pub hair: Option<[u8; 3]>,
}

/// The converted animations of an actor, by content hash of the source
/// `.kf`: `animations/<hash>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorAnimations {
    /// The standing idle.
    pub stand: String,
    /// The forward walk.
    pub walk: String,
    /// Every further animation that converted, by slot name
    /// ([`crate::actors::human_animation_slots`]: `run`, `hit_front`,
    /// `1hp.attack`, ...). Empty in a document written before slots
    /// existed; a renderer then has only the stand and the walk.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub more: BTreeMap<String, String>,
}

/// A weapon an actor holds or can be shown holding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorWeapon {
    /// The weapon record; `None` for a default model chosen by class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<FormKey>,
    /// Its editor id, as stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    /// The model path, normalised.
    pub path: String,
    /// Content hash of the model (`meshes/<hash>.glb`); `None` when it is
    /// missing or does not convert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    /// The weapon's animation type as stored in its `DNAM` (0 hand to
    /// hand, 1 one-hand melee, 2 two-hand melee, 3 and 4 pistols, 5 rifle,
    /// 6 automatic rifle, 7 energy rifle, 8 heavy, 9 launcher, 10 to 13
    /// thrown and mines); for a default, the type its family stands for.
    pub animation_type: u32,
    /// The source's animation family the type selects
    /// ([`crate::actors::weapon_family`]): the file prefix of its
    /// animations (`1hp`, `2hr`, ...).
    pub family: String,
    /// Whether the actor carries it (its first weapon in inventory
    /// order); `false` for a default model of a family.
    pub carried: bool,
}

/// `actors/<form>.json`: one placed actor base after its templates and
/// leveled lists are resolved (`docs/contracts.md`, section 10.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorDoc {
    /// [`SCHEMA_VERSION`] of the writer.
    pub schema_version: u32,
    /// The base form placed by the references: an NPC, a creature or a
    /// leveled list of them.
    pub form: FormKey,
    /// The base form's editor id, as stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_id: Option<String>,
    /// The display name, after templates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// NPC or creature.
    pub kind: ActorKind,
    /// Whether the actor is female, after templates.
    pub female: bool,
    /// The race, for an NPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race: Option<FormKey>,
    /// Content hash of the skeleton's source file: `skeletons/<hash>.json`.
    /// `None` when the skeleton is missing or does not convert; the notes
    /// and the manifest say which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skeleton: Option<String>,
    /// The skeleton's model path, normalised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skeleton_path: Option<String>,
    /// The meshes of the body, sorted by role, then path.
    pub parts: Vec<ActorPart>,
    /// The converted animations (`animations/<hash>.json`); `None` unless
    /// both the stand and the walk converted. A failure is in the notes
    /// and the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<ActorAnimations>,
    /// The standing idle's path, normalised; humans only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stand_path: Option<String>,
    /// The forward walk's path for the actor's sex, normalised; humans only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk_path: Option<String>,
    /// The further animations found in the data, by slot: the path the
    /// slot table chose for the actor's game and sex, normalised; humans
    /// only. The converted ones are in [`ActorAnimations::more`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub animation_paths: BTreeMap<String, String>,
    /// The weapon the actor carries, if any, then a default model for each
    /// default family ([`crate::actors::DEFAULT_WEAPONS`]) found in the
    /// data; humans only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weapons: Vec<ActorWeapon>,
    /// Colours.
    pub tint: ActorTint,
    /// Equal for two actors exactly when their parts (meshes by content,
    /// or by path when not converted), texture overrides, hair tint and sex
    /// are equal: a SHA-256 of those, lowercase hex.
    pub appearance: String,
    /// Base health, as stored, after templates.
    pub health: i32,
    /// Level, after templates. For a level relative to the player's, the
    /// stored minimum level.
    pub level: i32,
    /// For each group of data taken from a template, the forms followed
    /// to reach the actor that supplies it, leveled lists included, in
    /// order: `traits`, `stats`, `model`, `base_data`, `inventory`. A
    /// placed leveled list is listed under `base`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub template_chains: BTreeMap<String, Vec<FormKey>>,
    /// Links that did not resolve, in the order they were met.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One actor document of the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorEntry {
    /// The file stem under `actors/`.
    pub id: String,
    /// The placed base form.
    pub form: FormKey,
    /// NPC or creature.
    pub kind: ActorKind,
}

/// Something an extract could not use: a record that does not decode, a
/// reference with a non-finite transform, a broken navmesh. It was skipped
/// and the extract went on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Issue {
    /// What it concerns: a form key, or a path.
    pub subject: String,
    /// Record signature, when it concerns a record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
    /// Byte offset of the record in its plugin, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// What went wrong.
    pub error: String,
}

/// `manifest.json`: the extract's inputs and the path → hash index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`SCHEMA_VERSION`] of the writer.
    pub schema_version: u32,
    /// The slice that was extracted.
    pub slice: Slice,
    /// How much of the raw store `records.sqlite` holds.
    pub raw_store: RawStore,
    /// The plugins, in load order.
    pub plugins: Vec<PluginEntry>,
    /// The archives, in lookup order.
    pub archives: Vec<ArchiveEntry>,
    /// The exterior cells written, sorted.
    pub cells: Vec<CellKey>,
    /// The interior cells written, sorted by id.
    pub interiors: Vec<InteriorEntry>,
    /// The actor documents written, sorted by form.
    #[serde(default)]
    pub actors: Vec<ActorEntry>,
    /// Normalised model path → what became of it.
    pub meshes: BTreeMap<String, AssetEntry>,
    /// Normalised texture path → what became of it.
    pub textures: BTreeMap<String, AssetEntry>,
    /// Mesh content hash → facts about the converted mesh.
    pub mesh_info: BTreeMap<String, MeshInfo>,
    /// Texture content hash → facts about the converted texture.
    pub texture_info: BTreeMap<String, TextureInfo>,
    /// Normalised skeleton path of an actor document → what became of it.
    #[serde(default)]
    pub skeletons: BTreeMap<String, AssetEntry>,
    /// Normalised animation path of an actor document → what became of it.
    #[serde(default)]
    pub animations: BTreeMap<String, AssetEntry>,
    /// Skeleton content hash → facts about the converted skeleton.
    #[serde(default)]
    pub skeleton_info: BTreeMap<String, SkeletonInfo>,
    /// Animation content hash → facts about the converted animation.
    #[serde(default)]
    pub animation_info: BTreeMap<String, AnimationInfo>,
    /// What the slice extraction skipped, sorted. Records that do not
    /// decode at all are listed in `records.sqlite` (`bad_records`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<Issue>,
    /// The engine's default land texture, normalised, as this extract
    /// resolved it ([`crate::default_land_texture`]): what a land layer
    /// with [`LandLayer::defaulted`] set draws. `None` in a manifest
    /// written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_land_texture: Option<String>,
    /// The game the plugin list belongs to, detected from its first
    /// plugin or given to [`crate::extract_for_game`]. A manifest written
    /// before it was recorded reads as New Vegas.
    #[serde(default = "legacy_game")]
    pub game: fnv_esm::GameVariant,
    /// [`RECORDS_REVISION`] of the `records.sqlite` beside it; 0 in a
    /// manifest written before it was recorded.
    #[serde(default)]
    pub records_revision: u32,
    /// Things worth knowing that are not problems, sorted: a persistent
    /// reference standing in a grid cell that has no cell record (it is
    /// left out), a cell whose land comes from the parent worldspace.
    /// Unlike [`Manifest::issues`] they do not count as issues.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Issue>,
}

impl Manifest {
    /// The converted mesh of a model path, normalising the path first.
    pub fn mesh_hash(&self, model: &str) -> Option<&str> {
        self.meshes
            .get(&crate::paths::normalize_mesh_path(model))
            .and_then(AssetEntry::converted_hash)
    }

    /// The converted texture of a texture path, normalising the path first.
    pub fn texture_hash(&self, path: &str) -> Option<&str> {
        self.textures
            .get(&crate::paths::normalize_texture_path(path))
            .and_then(AssetEntry::converted_hash)
    }

    /// The converted skeleton of a skeleton path, normalising the path
    /// first (as a model path: `meshes\` is added when missing).
    pub fn skeleton_hash(&self, path: &str) -> Option<&str> {
        self.skeletons
            .get(&crate::paths::normalize_mesh_path(path))
            .and_then(AssetEntry::converted_hash)
    }

    /// The converted animation of a `.kf` path, normalising the path first
    /// (as a model path: `meshes\` is added when missing).
    pub fn animation_hash(&self, path: &str) -> Option<&str> {
        self.animations
            .get(&crate::paths::normalize_mesh_path(path))
            .and_then(AssetEntry::converted_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    fn rotated(rotation: [f32; 3], point: [f32; 3]) -> [f32; 3] {
        Transform {
            position: [0.0; 3],
            rotation,
            scale: 1.0,
        }
        .apply(point)
    }

    #[test]
    fn identity_matrix() {
        assert_eq!(
            Transform::default().matrix(),
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0]
            ]
        );
    }

    #[test]
    fn scale_and_translation() {
        let t = Transform {
            position: [10.0, -20.0, 30.0],
            rotation: [0.0; 3],
            scale: 2.5,
        };
        assert_eq!(
            t.matrix(),
            [
                [2.5, 0.0, 0.0, 10.0],
                [0.0, 2.5, 0.0, -20.0],
                [0.0, 0.0, 2.5, 30.0]
            ]
        );
        assert_eq!(t.apply([1.0, 2.0, 3.0]), [12.5, -15.0, 37.5]);
    }

    /// Positive angles are clockwise looking down the axis towards the
    /// origin, so each axis turns the other two the "wrong" way round for a
    /// right-handed frame.
    #[test]
    fn quarter_turns_about_each_axis() {
        // About Z, seen from above: east -> south, north -> east.
        assert!(close(
            rotated([0.0, 0.0, FRAC_PI_2], [1.0, 0.0, 0.0]),
            [0.0, -1.0, 0.0]
        ));
        assert!(close(
            rotated([0.0, 0.0, FRAC_PI_2], [0.0, 1.0, 0.0]),
            [1.0, 0.0, 0.0]
        ));
        // About X: north -> down, up -> north.
        assert!(close(
            rotated([FRAC_PI_2, 0.0, 0.0], [0.0, 1.0, 0.0]),
            [0.0, 0.0, -1.0]
        ));
        assert!(close(
            rotated([FRAC_PI_2, 0.0, 0.0], [0.0, 0.0, 1.0]),
            [0.0, 1.0, 0.0]
        ));
        // About Y: up -> west, east -> up.
        assert!(close(
            rotated([0.0, FRAC_PI_2, 0.0], [0.0, 0.0, 1.0]),
            [-1.0, 0.0, 0.0]
        ));
        assert!(close(
            rotated([0.0, FRAC_PI_2, 0.0], [1.0, 0.0, 0.0]),
            [0.0, 0.0, 1.0]
        ));
        // Half a turn about Z.
        assert!(close(
            rotated([0.0, 0.0, PI], [1.0, 2.0, 3.0]),
            [-1.0, -2.0, 3.0]
        ));
    }

    /// `R = Rz(-rz) · Ry(-ry) · Rx(-rx)`: X is applied first.
    #[test]
    fn axis_order_is_x_then_y_then_z() {
        // rx = 90° sends north to down; rz = 90° then leaves down alone.
        let both = [FRAC_PI_2, 0.0, FRAC_PI_2];
        assert!(close(rotated(both, [0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]));
        // rx = 90° sends up to north; rz = 90° then sends north to east.
        assert!(close(rotated(both, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
        // East is untouched by rx, then goes south.
        assert!(close(rotated(both, [1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]));
    }

    #[test]
    fn matrix_of_a_thirty_degree_yaw_with_scale() {
        let t = Transform {
            position: [100.0, 200.0, 300.0],
            rotation: [0.0, 0.0, PI / 6.0],
            scale: 2.0,
        };
        let (c, s) = (3f32.sqrt() / 2.0, 0.5f32);
        let expected = [
            [2.0 * c, 2.0 * s, 0.0, 100.0],
            [-2.0 * s, 2.0 * c, 0.0, 200.0],
            [0.0, 0.0, 2.0, 300.0],
        ];
        let m = t.matrix();
        for (got, want) in m.iter().flatten().zip(expected.iter().flatten()) {
            assert!((got - want).abs() < 1e-5, "{got} != {want}");
        }
    }

    #[test]
    fn cell_of_rounds_down() {
        assert_eq!(cell_of(&[0.0, 0.0, 0.0]), (0, 0));
        assert_eq!(cell_of(&[4095.9, -0.1, 0.0]), (0, -1));
        assert_eq!(cell_of(&[-4096.0, 8192.0, 0.0]), (-1, 2));
    }

    /// A document written before `trigger_volume` existed reads as false,
    /// and a false flag writes the same bytes as before.
    #[test]
    fn trigger_volume_is_optional_in_json() {
        let old = r#"{"form":"a.esm:000900","kind":"object","group":"temporary","base":"a.esm:000850","transform":{"position":[1.0,2.0,3.0],"rotation":[0.0,0.0,0.5],"scale":1.0},"flags":{"persistent":false,"initially_disabled":false,"deleted":false}}"#;
        let mut reference: Reference = serde_json::from_str(old).unwrap();
        assert!(!reference.trigger_volume);
        assert_eq!(serde_json::to_string(&reference).unwrap(), old);
        reference.trigger_volume = true;
        let flagged = serde_json::to_string(&reference).unwrap();
        assert!(flagged.ends_with(r#","trigger_volume":true}"#));
        let back: Reference = serde_json::from_str(&flagged).unwrap();
        assert_eq!(back, reference);
    }

    /// A document without a primitive reads as `None` and writes the same
    /// bytes; the shapes round-trip, and a code no layout is known for is
    /// kept.
    #[test]
    fn primitive_is_optional_and_keeps_unknown_shapes() {
        let old = r#"{"form":"a.esm:000900","kind":"object","group":"temporary","base":"a.esm:000850","transform":{"position":[1.0,2.0,3.0],"rotation":[0.0,0.0,0.5],"scale":1.0},"flags":{"persistent":false,"initially_disabled":false,"deleted":false}}"#;
        let mut reference: Reference = serde_json::from_str(old).unwrap();
        assert_eq!(reference.primitive, None);
        assert_eq!(serde_json::to_string(&reference).unwrap(), old);
        for (code, text) in [
            (1, r#""box""#),
            (2, r#""sphere""#),
            (3, r#""portal_box""#),
            (9, r#"{"other":9}"#),
        ] {
            let shape = PrimitiveShape::from_code(code);
            assert_eq!(serde_json::to_string(&shape).unwrap(), text);
            assert_eq!(serde_json::from_str::<PrimitiveShape>(text).unwrap(), shape);
        }
        reference.primitive = Some(PrimitiveDoc {
            bounds: [64.0, 32.0, 16.0],
            shape: PrimitiveShape::Box,
        });
        let text = serde_json::to_string(&reference).unwrap();
        assert!(text.ends_with(r#","primitive":{"bounds":[64.0,32.0,16.0],"shape":"box"}}"#));
        assert_eq!(serde_json::from_str::<Reference>(&text).unwrap(), reference);
    }
}
