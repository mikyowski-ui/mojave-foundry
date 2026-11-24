//! Building the cell documents of a slice: its exterior cells, with the
//! worldspace's persistent references assigned by position, and the
//! interiors reached through teleport doors.
//!
//! Model and texture hashes are left empty here; the extractor fills them
//! in once the assets are converted.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use fnv_esm::{CellChildKind, FormId, PlacedKind, PlacedRef, Record, sigs};

use crate::error::MirError;
use crate::finite;
use crate::index::{Entry, Index, Key};
use crate::model::{
    CellDoc, CellLocation, CellRef, EnableParentDoc, InteriorEntry, Issue, LandDefault, LandDoc,
    LandLayer, LayerOpacity, LockDoc, NavDoorLinkDoc, NavExternalLinkDoc, NavTriangleDoc,
    NavmeshDoc, RefFlags, RefGroup, RefKind, Reference, SCHEMA_VERSION, Slice, TeleportDoc,
    Transform, cell_of,
};
use crate::paths::{FormKey, normalize_mesh_path, normalize_texture_path};

/// The cell documents of one slice.
#[derive(Debug, Default)]
pub(crate) struct CellBuild {
    /// Lowercase editor id of the worldspace.
    pub worldspace: String,
    /// Exterior cells, sorted by `(x, y)`.
    pub exteriors: Vec<CellDoc>,
    /// Interior cells, sorted by id.
    pub interiors: Vec<(InteriorEntry, CellDoc)>,
    pub issues: Vec<Issue>,
    /// Facts worth reporting that are not problems ([`crate::Manifest::notes`]).
    pub notes: Vec<Issue>,
}

/// The bit of a worldspace's parent-use flags (`PNAM`) that says its land
/// comes from the parent worldspace (`WNAM`).
pub(crate) const USE_PARENT_LAND: u16 = 0x0001;

/// What is known about a cell record.
#[derive(Debug, Clone)]
struct CellInfo {
    form: FormKey,
    editor_id: Option<String>,
    name: Option<String>,
    interior: bool,
    grid: Option<(i32, i32)>,
    world: Option<Key>,
    /// A worldspace's own persistent cell, outside every block.
    persistent: bool,
    flags: u8,
    water_height: Option<f32>,
}

#[derive(Debug, Clone)]
struct BaseInfo {
    sig: String,
    editor_id: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct LandTextureInfo {
    /// The form is a land texture record that decodes.
    found: bool,
    editor_id: Option<String>,
    texture: Option<String>,
}

/// A reference and the destination door of its teleport, if any.
struct Built {
    reference: Reference,
    teleport_door: Option<Key>,
}

struct Builder<'i, 'a> {
    index: &'i Index<'a>,
    /// Cell → the land, navmesh and reference entries in its children
    /// groups, by entry position, in form order.
    children: BTreeMap<Key, Vec<usize>>,
    cells: BTreeMap<Key, Option<CellInfo>>,
    bases: BTreeMap<Key, BaseInfo>,
    land_textures: BTreeMap<Key, LandTextureInfo>,
    /// The engine's default land texture, normalised.
    default_land_texture: String,
    worlds: BTreeMap<Key, String>,
    issues: Vec<Issue>,
    notes: Vec<Issue>,
}

fn is_placed(sig: fnv_esm::Sig) -> bool {
    sig == sigs::REFR || sig == sigs::ACHR || sig == sigs::ACRE
}

/// Base record types whose references become trigger volumes when they
/// carry a primitive: activators and talking activators.
const TRIGGER_BASES: &[&str] = &["ACTI", "TACT"];

/// Whether a reference is an invisible trigger or activator volume: a
/// primitive (`XPRM`) on a reference to an activator. The same base placed
/// without a primitive is an ordinary, visible activator, and a primitive
/// on a marker (`STAT` room, collision and multibound markers), an
/// acoustic space or a sound is not a trigger.
fn is_trigger_volume(placed: &PlacedRef, base_sig: Option<&str>) -> bool {
    placed.primitive.is_some() && base_sig.is_some_and(|sig| TRIGGER_BASES.contains(&sig))
}

/// A file stem for an interior: the lowercase editor id with anything but
/// ASCII letters, digits, `_` and `-` replaced by `_`.
pub(crate) fn interior_id(editor_id: Option<&str>, form: &FormKey) -> String {
    let raw = match editor_id.map(str::trim).filter(|e| !e.is_empty()) {
        Some(editor_id) => editor_id.to_ascii_lowercase(),
        None => format!("cell_{}_{:06x}", form.plugin, form.local_id),
    };
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl<'i, 'a> Builder<'i, 'a> {
    fn new(index: &'i Index<'a>, default_land_texture: &str) -> Self {
        let mut children: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
        for (position, entry) in index.entries().iter().enumerate() {
            let sig = entry.sig();
            if let Some(cell) = entry.cell
                && (is_placed(sig) || sig == sigs::LAND || sig == sigs::NAVM)
            {
                children.entry(cell).or_default().push(position);
            }
        }
        Self {
            index,
            children,
            cells: BTreeMap::new(),
            bases: BTreeMap::new(),
            land_textures: BTreeMap::new(),
            default_land_texture: default_land_texture.to_owned(),
            worlds: BTreeMap::new(),
            issues: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn note(&mut self, entry: &Entry<'_>, text: impl Into<String>) {
        self.notes.push(Issue {
            subject: self.index.form_key(entry.key).to_string(),
            sig: Some(entry.sig().to_string()),
            offset: Some(entry.record.offset as u64),
            error: text.into(),
        });
    }

    /// The first `LAND` entry among a cell's children.
    fn land_entry(&self, cell: Key) -> Option<Entry<'a>> {
        let index = self.index;
        self.children
            .get(&cell)?
            .iter()
            .map(|&position| index.entries()[position])
            .find(|e| e.sig() == sigs::LAND)
    }

    /// The parent worldspace whose land a worldspace uses: its `WNAM`, when
    /// its `PNAM` has [`USE_PARENT_LAND`] set and the parent is in the list.
    fn land_parent(&mut self, world: Key) -> Option<Key> {
        let index = self.index;
        let entry = index.get(world).copied()?;
        let Some(Record::World(record)) = self.decode(&entry) else {
            return None;
        };
        let parent = record.parent.and_then(|p| index.resolve(entry.plugin, p))?;
        let uses_land = record
            .parent_flags
            .is_some_and(|flags| flags & USE_PARENT_LAND != 0);
        (uses_land && index.get(parent).is_some()).then_some(parent)
    }

    fn issue(&mut self, entry: &Entry<'_>, error: impl Into<String>) {
        self.issues.push(Issue {
            subject: self.index.form_key(entry.key).to_string(),
            sig: Some(entry.sig().to_string()),
            offset: Some(entry.record.offset as u64),
            error: error.into(),
        });
    }

    fn decode(&mut self, entry: &Entry<'a>) -> Option<Record> {
        match self.index.decode(entry) {
            Ok(record) => Some(record),
            Err(bad) => {
                self.issues.push(Issue {
                    subject: bad.form.map_or_else(String::new, |f| f.to_string()),
                    sig: Some(bad.sig),
                    offset: Some(bad.offset),
                    error: bad.error,
                });
                None
            }
        }
    }

    fn world_name(&mut self, key: Key) -> String {
        if let Some(name) = self.worlds.get(&key) {
            return name.clone();
        }
        let name = self
            .index
            .summary(key)
            .and_then(|s| s.editor_id)
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_else(|| self.index.form_key(key).to_string());
        self.worlds.insert(key, name.clone());
        name
    }

    fn cell_info(&mut self, key: Key) -> Option<CellInfo> {
        if let Some(info) = self.cells.get(&key) {
            return info.clone();
        }
        let index = self.index;
        let info = match index.get(key).copied() {
            Some(entry) if entry.sig() == sigs::CELL => match self.decode(&entry) {
                Some(Record::Cell(cell)) => Some(CellInfo {
                    form: index.form_key(key),
                    interior: cell.is_interior(),
                    grid: cell.grid.map(|g| (g.x, g.y)),
                    world: entry.world,
                    persistent: entry.world.is_some() && !entry.in_block,
                    flags: cell.flags.unwrap_or(0),
                    water_height: cell.water_height.filter(|h| h.is_finite()),
                    editor_id: cell.common.editor_id,
                    name: cell.common.full_name,
                }),
                _ => None,
            },
            _ => None,
        };
        self.cells.insert(key, info.clone());
        info
    }

    fn base_info(&mut self, key: Key) -> Option<BaseInfo> {
        if let Some(info) = self.bases.get(&key) {
            return Some(info.clone());
        }
        let summary = self.index.summary(key)?;
        let info = BaseInfo {
            sig: summary.sig,
            editor_id: summary.editor_id,
            model: summary
                .model
                .as_deref()
                .map(normalize_mesh_path)
                .filter(|m| !m.is_empty()),
        };
        self.bases.insert(key, info.clone());
        Some(info)
    }

    fn land_texture(&mut self, plugin: u16, form: FormId) -> (Option<Key>, LandTextureInfo) {
        let Some(key) = self.index.resolve(plugin, form) else {
            return (None, LandTextureInfo::default());
        };
        if let Some(info) = self.land_textures.get(&key) {
            return (Some(key), info.clone());
        }
        let index = self.index;
        let mut info = LandTextureInfo::default();
        if let Some(entry) = index.get(key).copied()
            && let Some(Record::LandTexture(ltex)) = self.decode(&entry)
        {
            info.found = true;
            info.editor_id = ltex.common.editor_id.clone();
            let set = ltex
                .texture_set
                .and_then(|set| index.resolve(entry.plugin, set))
                .and_then(|set| index.get(set).copied());
            if let Some(set_entry) = set
                && let Some(Record::TextureSet(set)) = self.decode(&set_entry)
            {
                info.texture = set
                    .diffuse()
                    .map(normalize_texture_path)
                    .filter(|p| !p.is_empty());
            }
        }
        self.land_textures.insert(key, info.clone());
        (Some(key), info)
    }

    /// The land texture of a layer, with the default land texture standing
    /// in, and the reason, when the layer gives no usable one: a null form,
    /// a form that is not a land texture of the plugin list, or a land
    /// texture with no diffuse texture.
    fn layer_texture(
        &mut self,
        entry: &Entry<'a>,
        form: FormId,
    ) -> (Option<Key>, LandTextureInfo, Option<LandDefault>) {
        let (key, mut info) = self.land_texture(entry.plugin, form);
        let reason = if form.is_null() {
            Some(LandDefault::NullTexture)
        } else if !info.found {
            self.issue(
                entry,
                "a land layer names a form that is not a land texture of the plugin list; \
                 the default land texture stands in",
            );
            Some(LandDefault::UnknownTexture)
        } else if info.texture.is_none() {
            self.issue(
                entry,
                format!(
                    "the land texture {} has no diffuse texture; the default land texture stands in",
                    key.map_or_else(String::new, |k| self.index.form_key(k).to_string())
                ),
            );
            Some(LandDefault::NoTextureSet)
        } else {
            None
        };
        if reason.is_some() {
            info.texture = Some(self.default_land_texture.clone());
        }
        (key, info, reason)
    }

    /// Where a cell is, for a teleport destination or a document. A door
    /// in a worldspace's persistent cell is placed in the grid cell its
    /// position falls in.
    fn location(&mut self, info: &CellInfo, position: Option<[f32; 3]>) -> CellLocation {
        if info.interior {
            return CellLocation::Interior;
        }
        let (worldspace, worldspace_form) = match info.world {
            Some(world) => (self.world_name(world), self.index.form_key(world)),
            None => (String::new(), info.form.clone()),
        };
        let (x, y) = match (info.persistent, position) {
            (true, Some(p)) => cell_of(&p),
            _ => info.grid.unwrap_or((0, 0)),
        };
        CellLocation::Exterior {
            worldspace,
            worldspace_form,
            x,
            y,
        }
    }

    fn reference(
        &mut self,
        entry: &Entry<'a>,
        placed: &PlacedRef,
        group: RefGroup,
    ) -> Option<Built> {
        let index = self.index;
        let plugin = entry.plugin;
        let Some(base_key) = placed.base.and_then(|b| index.resolve(plugin, b)) else {
            self.issue(entry, "the reference has no base form in the plugin list");
            return None;
        };
        let Some(placement) = placed.placement else {
            self.issue(entry, "the reference has no position (DATA)");
            return None;
        };
        let transform = Transform {
            position: placement.position,
            rotation: placement.rotation,
            scale: placed.scale,
        };
        if !transform.is_finite() {
            self.issue(entry, "the reference's transform holds a non-finite number");
            return None;
        }
        let base = self.base_info(base_key);
        let mut teleport = None;
        let mut teleport_door = None;
        if let Some(t) = placed.teleport {
            match index.resolve(plugin, t.door) {
                Some(door) if finite::all(&t.position) && finite::all(&t.rotation) => {
                    let cell = self.door_cell(door);
                    teleport_door = Some(door);
                    teleport = Some(TeleportDoc {
                        door: index.form_key(door),
                        cell,
                        position: t.position,
                        rotation: t.rotation,
                    });
                }
                Some(_) => self.issue(entry, "the teleport holds a non-finite number"),
                None => self.issue(entry, "the teleport names a door outside the plugin list"),
            }
        }
        let key = |f: Option<FormId>| f.and_then(|f| index.form(plugin, f));
        let flags = placed.common.meta.flags;
        let trigger_volume = is_trigger_volume(placed, base.as_ref().map(|b| b.sig.as_str()));
        let reference = Reference {
            form: index.form_key(entry.key),
            editor_id: placed.common.editor_id.clone(),
            kind: match placed.kind {
                PlacedKind::Object => RefKind::Object,
                PlacedKind::Npc => RefKind::Npc,
                PlacedKind::Creature => RefKind::Creature,
            },
            group,
            base: index.form_key(base_key),
            base_kind: base.as_ref().map(|b| b.sig.clone()),
            base_editor_id: base.as_ref().and_then(|b| b.editor_id.clone()),
            model: base.and_then(|b| b.model),
            mesh: None,
            transform,
            flags: RefFlags {
                persistent: flags.is_persistent(),
                initially_disabled: flags.is_initially_disabled(),
                deleted: flags.is_deleted(),
            },
            teleport,
            enable_parent: placed.enable_parent.and_then(|e| {
                Some(EnableParentDoc {
                    reference: index.form(plugin, e.reference)?,
                    opposite: e.is_opposite(),
                })
            }),
            lock: placed.lock.map(|l| LockDoc {
                level: l.level,
                key: key(Some(l.key)),
                flags: l.flags,
            }),
            count: placed.count,
            linked_ref: key(placed.linked_ref),
            owner: key(placed.owner),
            trigger_volume,
        };
        Some(Built {
            reference,
            teleport_door,
        })
    }

    /// The cell a destination door stands in.
    fn door_cell(&mut self, door: Key) -> Option<CellRef> {
        let entry = self.index.get(door).copied()?;
        let cell_key = entry.cell?;
        let info = self.cell_info(cell_key)?;
        let position = if info.persistent {
            match self.decode(&entry) {
                Some(Record::Placed(p)) => p.placement.map(|p| p.position),
                _ => None,
            }
            .filter(|p| finite::all(p))
        } else {
            None
        };
        let location = self.location(&info, position);
        Some(CellRef {
            form: info.form.clone(),
            editor_id: info.editor_id.clone(),
            location,
        })
    }

    fn land(&mut self, entry: &Entry<'a>) -> Option<LandDoc> {
        let Some(Record::Land(land)) = self.decode(entry) else {
            return None;
        };
        let heights = land.heights.clone()?;
        if heights.len() != fnv_esm::LAND_VERTICES || !finite::all(&heights) {
            self.issue(entry, "the land height grid holds a non-finite number");
            return None;
        }
        let mut layers: Vec<(usize, LandLayer)> = Vec::new();
        for base in &land.base_layers {
            let (key, info, defaulted) = self.layer_texture(entry, base.texture);
            layers.push((
                layers.len(),
                LandLayer {
                    quadrant: base.quadrant,
                    layer: -1,
                    land_texture: key.map(|k| self.index.form_key(k)),
                    land_texture_editor_id: info.editor_id,
                    texture: info.texture,
                    texture_hash: None,
                    opacity: Vec::new(),
                    defaulted,
                },
            ));
        }
        // A quadrant that stores no base layer is drawn with the default
        // land texture under its blended layers.
        for quadrant in land.quadrants_without_base() {
            layers.push((
                layers.len(),
                LandLayer {
                    quadrant,
                    layer: -1,
                    land_texture: None,
                    land_texture_editor_id: None,
                    texture: Some(self.default_land_texture.clone()),
                    texture_hash: None,
                    opacity: Vec::new(),
                    defaulted: Some(LandDefault::NoBaseLayer),
                },
            ));
        }
        for alpha in &land.alpha_layers {
            let (key, info, defaulted) = self.layer_texture(entry, alpha.texture);
            let opacity: Vec<LayerOpacity> = alpha
                .opacity
                .iter()
                .filter(|p| p.opacity.is_finite())
                .map(|p| LayerOpacity {
                    position: p.position,
                    opacity: p.opacity,
                })
                .collect();
            if opacity.len() != alpha.opacity.len() {
                self.issue(
                    entry,
                    "a land layer opacity is non-finite; the point is dropped",
                );
            }
            layers.push((
                layers.len(),
                LandLayer {
                    quadrant: alpha.quadrant,
                    layer: i32::from(alpha.layer),
                    land_texture: key.map(|k| self.index.form_key(k)),
                    land_texture_editor_id: info.editor_id,
                    texture: info.texture,
                    texture_hash: None,
                    opacity,
                    defaulted,
                },
            ));
        }
        layers.sort_by_key(|(order, l)| (l.quadrant, l.layer, *order));
        let full = |n: usize| n == fnv_esm::LAND_VERTICES;
        Some(LandDoc {
            form: self.index.form_key(entry.key),
            heights,
            normals: land.normals.filter(|n| full(n.len())),
            colors: land.colors.filter(|c| full(c.len())),
            layers: layers.into_iter().map(|(_, l)| l).collect(),
        })
    }

    fn navmesh(&mut self, entry: &Entry<'a>) -> Option<NavmeshDoc> {
        let Some(Record::Navmesh(nav)) = self.decode(entry) else {
            return None;
        };
        if !finite::points(&nav.vertices) {
            self.issue(entry, "the navmesh has a non-finite vertex; it is skipped");
            return None;
        }
        let count = nav.vertices.len();
        let mut triangles = Vec::with_capacity(nav.triangles.len());
        for t in &nav.triangles {
            let mut vertices = [0u32; 3];
            for (out, &v) in vertices.iter_mut().zip(&t.vertices) {
                match usize::try_from(v) {
                    Ok(v) if v < count => *out = v as u32,
                    _ => {
                        self.issue(
                            entry,
                            "a navmesh triangle names a vertex out of range; the navmesh is skipped",
                        );
                        return None;
                    }
                }
            }
            triangles.push(NavTriangleDoc {
                vertices,
                links: t.edges.map(i32::from),
                flags: t.flags,
            });
        }
        let index = self.index;
        let plugin = entry.plugin;
        Some(NavmeshDoc {
            form: index.form_key(entry.key),
            vertices: nav.vertices,
            triangles,
            cover_triangles: nav.cover_triangles.iter().map(|&t| u32::from(t)).collect(),
            door_links: nav
                .door_links
                .iter()
                .filter_map(|l| {
                    Some(NavDoorLinkDoc {
                        door: index.form(plugin, l.door)?,
                        triangle: u32::from(l.triangle),
                    })
                })
                .collect(),
            external_links: nav
                .external_links
                .iter()
                .filter_map(|l| {
                    Some(NavExternalLinkDoc {
                        navmesh: index.form(plugin, l.navmesh)?,
                        triangle: u32::from(l.triangle),
                    })
                })
                .collect(),
        })
    }

    /// The document of a cell from its children groups, with the
    /// destination doors of its teleports.
    fn cell_doc(&mut self, key: Key, info: &CellInfo, with_land: bool) -> (CellDoc, Vec<Key>) {
        let index = self.index;
        let location = self.location(info, None);
        let mut doc = CellDoc {
            schema_version: SCHEMA_VERSION,
            form: info.form.clone(),
            editor_id: info.editor_id.clone(),
            name: info.name.clone(),
            location,
            flags: info.flags,
            water_height: info.water_height,
            land: None,
            references: Vec::new(),
            navmeshes: Vec::new(),
        };
        let mut doors = Vec::new();
        let children = self.children.get(&key).cloned().unwrap_or_default();
        for position in children {
            let entry = index.entries()[position];
            let sig = entry.sig();
            if sig == sigs::LAND {
                if with_land && doc.land.is_none() {
                    doc.land = self.land(&entry);
                }
            } else if sig == sigs::NAVM {
                if let Some(nav) = self.navmesh(&entry) {
                    doc.navmeshes.push(nav);
                }
            } else if let Some(Record::Placed(placed)) = self.decode(&entry) {
                let group = match entry.child {
                    Some(CellChildKind::Persistent) => RefGroup::Persistent,
                    Some(CellChildKind::VisibleDistant) => RefGroup::VisibleDistant,
                    _ => RefGroup::Temporary,
                };
                if let Some(built) = self.reference(&entry, &placed, group) {
                    doors.extend(built.teleport_door);
                    doc.references.push(built.reference);
                }
            }
        }
        (doc, doors)
    }
}

/// Builds the documents of the slice.
/// `default_land_texture` is the normalised path that stands in for a land
/// layer with no usable land texture.
pub(crate) fn build(
    index: &Index<'_>,
    slice: &Slice,
    default_land_texture: &str,
) -> Result<CellBuild, MirError> {
    let mut builder = Builder::new(index, default_land_texture);
    let wanted = slice.worldspace.trim();
    let world = index
        .entries()
        .iter()
        .filter(|e| e.sig() == sigs::WRLD)
        .find(|e| {
            index
                .summary(e.key)
                .and_then(|s| s.editor_id)
                .is_some_and(|id| id.eq_ignore_ascii_case(wanted))
        })
        .map(|e| e.key)
        .ok_or_else(|| MirError::WorldspaceNotFound(slice.worldspace.clone()))?;
    let worldspace = builder.world_name(world);

    // The worldspace's cells: the grid cells of the slice and the
    // persistent cell.
    let mut grid: BTreeMap<(i32, i32), (Key, CellInfo)> = BTreeMap::new();
    let mut persistent_cells = Vec::new();
    let cell_keys: Vec<Key> = index
        .entries()
        .iter()
        .filter(|e| e.sig() == sigs::CELL && e.world == Some(world))
        .map(|e| e.key)
        .collect();
    for key in cell_keys {
        let Some(info) = builder.cell_info(key) else {
            continue;
        };
        if info.persistent {
            persistent_cells.push(key);
        } else if let Some((x, y)) = info.grid
            && slice.contains(x, y)
        {
            // Two records for one grid cell: the first form wins.
            grid.entry((x, y)).or_insert((key, info));
        }
    }

    let mut docs: BTreeMap<(i32, i32), CellDoc> = BTreeMap::new();
    let mut frontier: Vec<Key> = Vec::new();
    for (&position, (key, info)) in &grid {
        let (doc, doors) = builder.cell_doc(*key, info, true);
        frontier.extend(doors);
        docs.insert(position, doc);
    }

    // A child worldspace that uses its parent's land: a cell with no land
    // record of its own takes the parent's at the same grid position.
    if let Some(parent) = builder.land_parent(world) {
        let parent_name = builder.world_name(parent);
        let mut parent_cells: BTreeMap<(i32, i32), Key> = BTreeMap::new();
        let keys: Vec<Key> = index
            .entries()
            .iter()
            .filter(|e| e.sig() == sigs::CELL && e.world == Some(parent))
            .map(|e| e.key)
            .collect();
        for key in keys {
            if let Some(info) = builder.cell_info(key)
                && !info.persistent
                && let Some((x, y)) = info.grid
                && slice.contains(x, y)
            {
                parent_cells.entry((x, y)).or_insert(key);
            }
        }
        for (&(x, y), (key, _)) in &grid {
            if builder.land_entry(*key).is_some() {
                continue;
            }
            let Some(land_entry) = parent_cells
                .get(&(x, y))
                .and_then(|&cell| builder.land_entry(cell))
            else {
                continue;
            };
            if let Some(land) = builder.land(&land_entry)
                && let Some(doc) = docs.get_mut(&(x, y))
            {
                let cell_entry = index.get(*key).copied();
                let text = format!(
                    "cell ({x}, {y}) has no land of its own; it takes the land of the parent \
                     worldspace {parent_name} at the same position ({})",
                    land.form
                );
                doc.land = Some(land);
                if let Some(cell_entry) = cell_entry {
                    builder.note(&cell_entry, text);
                }
            }
        }
    }

    // Persistent references stand in the grid cell of their position.
    for cell in persistent_cells {
        let children = builder.children.get(&cell).cloned().unwrap_or_default();
        for position in children {
            let entry = index.entries()[position];
            if !is_placed(entry.sig()) {
                continue;
            }
            let Some(Record::Placed(placed)) = builder.decode(&entry) else {
                continue;
            };
            let Some(place) = placed.placement.filter(|p| finite::all(&p.position)) else {
                continue;
            };
            let (x, y) = cell_of(&place.position);
            if !slice.contains(x, y) {
                continue;
            }
            if !docs.contains_key(&(x, y)) {
                builder.note(
                    &entry,
                    format!(
                        "persistent reference stands in cell ({x}, {y}), which has no cell \
                         record; it is left out"
                    ),
                );
                continue;
            }
            if let Some(built) = builder.reference(&entry, &placed, RefGroup::WorldPersistent)
                && let Some(doc) = docs.get_mut(&(x, y))
            {
                frontier.extend(built.teleport_door);
                doc.references.push(built.reference);
            }
        }
    }

    // Interiors, breadth first from the slice's teleport doors.
    let mut interiors: Vec<(InteriorEntry, CellDoc)> = Vec::new();
    let mut seen: BTreeSet<Key> = BTreeSet::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<(Key, u32)> = VecDeque::new();
    let enqueue = |builder: &mut Builder<'_, '_>,
                   doors: Vec<Key>,
                   depth: u32,
                   queue: &mut VecDeque<(Key, u32)>,
                   seen: &mut BTreeSet<Key>| {
        let mut cells: Vec<Key> = doors
            .into_iter()
            .filter_map(|door| index.get(door).and_then(|e| e.cell))
            .collect();
        cells.sort();
        cells.dedup();
        for cell in cells {
            let interior = builder.cell_info(cell).is_some_and(|i| i.interior);
            if interior && seen.insert(cell) {
                queue.push_back((cell, depth));
            }
        }
    };
    if slice.interior_depth > 0 {
        enqueue(&mut builder, frontier, 1, &mut queue, &mut seen);
    }
    while let Some((cell, depth)) = queue.pop_front() {
        let Some(info) = builder.cell_info(cell) else {
            continue;
        };
        let (doc, doors) = builder.cell_doc(cell, &info, false);
        let mut id = interior_id(info.editor_id.as_deref(), &info.form);
        if !ids.insert(id.clone()) {
            id = format!("{id}_{:06x}", info.form.local_id);
            ids.insert(id.clone());
        }
        interiors.push((
            InteriorEntry {
                id,
                form: info.form.clone(),
                depth,
            },
            doc,
        ));
        if depth < slice.interior_depth {
            enqueue(&mut builder, doors, depth + 1, &mut queue, &mut seen);
        }
    }

    let mut exteriors: Vec<CellDoc> = docs.into_values().collect();
    for doc in exteriors
        .iter_mut()
        .chain(interiors.iter_mut().map(|(_, d)| d))
    {
        doc.references.sort_by(|a, b| a.form.cmp(&b.form));
        doc.references.dedup_by(|a, b| a.form == b.form);
        doc.navmeshes.sort_by(|a, b| a.form.cmp(&b.form));
    }
    interiors.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    let mut issues = builder.issues;
    issues.sort();
    issues.dedup();
    let mut notes = builder.notes;
    notes.sort();
    notes.dedup();
    Ok(CellBuild {
        worldspace,
        exteriors,
        interiors,
        issues,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fnv_esm::Primitive;

    #[test]
    fn only_a_primitive_on_an_activator_is_a_trigger_volume() {
        let plain = PlacedRef::new(PlacedKind::Object, 0x900u32, 0x850u32, [0.0; 3], [0.0; 3]);
        let volume = plain.clone().with_primitive(Primitive {
            bounds: [64.0; 3],
            shape: Primitive::SHAPE_BOX,
            ..Primitive::default()
        });
        assert!(is_trigger_volume(&volume, Some("ACTI")));
        assert!(is_trigger_volume(&volume, Some("TACT")));
        assert!(!is_trigger_volume(&plain, Some("ACTI")));
        for marker in ["STAT", "ASPC", "SOUN"] {
            assert!(!is_trigger_volume(&volume, Some(marker)));
        }
        assert!(!is_trigger_volume(&volume, None));
    }
}
