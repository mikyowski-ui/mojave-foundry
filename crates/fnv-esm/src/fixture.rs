//! Synthetic plugins for tests. Always compiled, so that other crates can
//! build their inputs with it; no test needs a game install.
//!
//! [`PluginBuilder`] writes a valid plugin: the `TES4` header with its
//! masters, top-level groups, and the full nesting of worldspaces, cells
//! and topics. A record whose flags have [`RecordFlags::COMPRESSED`] is
//! written compressed, and a subrecord of 64 KiB or more gets the `XXXX`
//! escape.
//!
//! Typed records are made with the constructors on the record types
//! ([`Static::new`](crate::Static::new), [`Cell::exterior`],
//! [`PlacedRef::new`], [`Land::with_heights`] and so on) or as struct
//! literals, and every one of them reads back equal to what was written.
//!
//! ```
//! use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec, flat_heights};
//! use fnv_esm::{Cell, Land, PlacedKind, PlacedRef, Plugin, Static, World};
//!
//! let mut builder = PluginBuilder::new();
//! builder.add(Static::new(0x1000, "TownWell", "town\\well.nif"));
//! builder.world(WorldSpec::new(World::new(0x2000, "TownWorld")).with_cell(
//!     CellSpec::new(Cell::exterior(0x2001, 0, 0))
//!         .with_land(Land::new(0x2002).with_heights(&flat_heights(64.0)))
//!         .with_temporary(PlacedRef::new(
//!             PlacedKind::Object,
//!             0x2003,
//!             0x1000,
//!             [2048.0, 2048.0, 64.0],
//!             [0.0, 0.0, 0.0],
//!         )),
//! ));
//! let bytes = builder.build();
//! assert_eq!(Plugin::parse(&bytes)?.header().masters.len(), 0);
//! # Ok::<(), fnv_esm::EsmError>(())
//! ```

use std::collections::BTreeMap;
use std::io::Write;

use crate::bytes::{Wr, zstring_bytes};
use crate::formid::FormId;
use crate::header::{CellChildKind, HEADER_LEN, RecordFlags};
use crate::records::{
    Cell, Info, LAND_VERTICES, Land, Navmesh, PlacedRef, RawRecord, Record, Topic, TypedRecord,
    World,
};
use crate::sig::{Sig, sigs};
use crate::subrecord::RawSubrecord;

/// The file format version this game's plugins declare in `HEDR`.
pub const PLUGIN_VERSION: f32 = 1.34;

/// The `HEDR` version Fallout 3 plugins declare; pass it to
/// [`PluginBuilder::version`] for a Fallout 3 fixture.
pub const FALLOUT3_PLUGIN_VERSION: f32 = 0.94;

/// A 33 × 33 height grid with every vertex at `height`.
pub fn flat_heights(height: f32) -> Vec<f32> {
    vec![height; LAND_VERTICES]
}

/// A 33 × 33 normal grid with every normal pointing straight up.
pub fn up_normals() -> Vec<[i8; 3]> {
    vec![[0, 0, 127]; LAND_VERTICES]
}

/// A 33 × 33 vertex colour grid of one colour.
pub fn solid_colors(rgb: [u8; 3]) -> Vec<[u8; 3]> {
    vec![rgb; LAND_VERTICES]
}

/// A record or a group, as it will be written.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A record.
    Record(RawRecord),
    /// A group and its children.
    Group(GroupNode),
}

impl From<RawRecord> for Node {
    fn from(record: RawRecord) -> Self {
        Node::Record(record)
    }
}

impl From<GroupNode> for Node {
    fn from(group: GroupNode) -> Self {
        Node::Group(group)
    }
}

impl From<Record> for Node {
    fn from(record: Record) -> Self {
        Node::Record(record.to_raw())
    }
}

impl<T: TypedRecord> From<T> for Node {
    fn from(record: T) -> Self {
        Node::Record(record.to_raw())
    }
}

/// A group to write: its type, label and children.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupNode {
    /// The group type.
    pub group_type: i32,
    /// The label.
    pub label: [u8; 4],
    /// The stamp field.
    pub stamp: u32,
    /// The group's contents, in order.
    pub children: Vec<Node>,
}

impl GroupNode {
    /// An empty group of any type.
    pub fn new(group_type: i32, label: [u8; 4]) -> Self {
        Self {
            group_type,
            label,
            stamp: 0,
            children: Vec::new(),
        }
    }

    /// A top-level group (type 0).
    pub fn top(sig: Sig) -> Self {
        Self::new(0, sig.0)
    }

    /// A worldspace's children (type 1).
    pub fn world_children(world: FormId) -> Self {
        Self::new(1, world.0.to_le_bytes())
    }

    /// An interior cell block (type 2).
    pub fn interior_block(number: i32) -> Self {
        Self::new(2, number.to_le_bytes())
    }

    /// An interior cell sub-block (type 3).
    pub fn interior_sub_block(number: i32) -> Self {
        Self::new(3, number.to_le_bytes())
    }

    /// An exterior cell block (type 4). The label stores Y first.
    pub fn exterior_block(x: i16, y: i16) -> Self {
        Self::new(4, grid_label(x, y))
    }

    /// An exterior cell sub-block (type 5). The label stores Y first.
    pub fn exterior_sub_block(x: i16, y: i16) -> Self {
        Self::new(5, grid_label(x, y))
    }

    /// A cell's children (type 6).
    pub fn cell_children(cell: FormId) -> Self {
        Self::new(6, cell.0.to_le_bytes())
    }

    /// A topic's children (type 7).
    pub fn topic_children(topic: FormId) -> Self {
        Self::new(7, topic.0.to_le_bytes())
    }

    /// One of a cell's persistent, temporary or visible-when-distant
    /// children groups (types 8, 9, 10).
    pub fn cell_child(kind: CellChildKind, cell: FormId) -> Self {
        let group_type = match kind {
            CellChildKind::Persistent => 8,
            CellChildKind::Temporary => 9,
            CellChildKind::VisibleDistant => 10,
        };
        Self::new(group_type, cell.0.to_le_bytes())
    }

    /// Appends a child.
    pub fn push(&mut self, node: impl Into<Node>) {
        self.children.push(node.into());
    }

    /// Appends a child, by value.
    #[must_use]
    pub fn with(mut self, node: impl Into<Node>) -> Self {
        self.push(node);
        self
    }
}

fn grid_label(x: i16, y: i16) -> [u8; 4] {
    let (x, y) = (x.to_le_bytes(), y.to_le_bytes());
    [y[0], y[1], x[0], x[1]]
}

/// A cell and everything in its children groups.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CellSpec {
    /// The cell record.
    pub cell: Cell,
    /// The cell's terrain; written first in the temporary children.
    pub land: Option<Land>,
    /// The cell's navmeshes; written after the land.
    pub navmeshes: Vec<Navmesh>,
    /// Persistent references.
    pub persistent: Vec<PlacedRef>,
    /// Temporary references.
    pub temporary: Vec<PlacedRef>,
    /// Visible-when-distant references.
    pub visible_distant: Vec<PlacedRef>,
    /// Any other records for the temporary children, written last.
    pub extra_temporary: Vec<RawRecord>,
}

impl CellSpec {
    /// A cell with no children.
    pub fn new(cell: Cell) -> Self {
        Self {
            cell,
            ..Self::default()
        }
    }

    /// Sets the terrain.
    #[must_use]
    pub fn with_land(mut self, land: Land) -> Self {
        self.land = Some(land);
        self
    }

    /// Adds a navmesh.
    #[must_use]
    pub fn with_navmesh(mut self, navmesh: Navmesh) -> Self {
        self.navmeshes.push(navmesh);
        self
    }

    /// Adds a persistent reference.
    #[must_use]
    pub fn with_persistent(mut self, reference: PlacedRef) -> Self {
        self.persistent.push(reference);
        self
    }

    /// Adds a temporary reference.
    #[must_use]
    pub fn with_temporary(mut self, reference: PlacedRef) -> Self {
        self.temporary.push(reference);
        self
    }

    /// Adds a visible-when-distant reference.
    #[must_use]
    pub fn with_visible_distant(mut self, reference: PlacedRef) -> Self {
        self.visible_distant.push(reference);
        self
    }

    /// The cell record followed, when it has children, by its children
    /// group.
    fn nodes(&self) -> Vec<Node> {
        let cell = self.cell.common.meta.form_id;
        let mut children = GroupNode::cell_children(cell);
        let mut child_group = |kind: CellChildKind, nodes: Vec<Node>| {
            if !nodes.is_empty() {
                let mut group = GroupNode::cell_child(kind, cell);
                group.children = nodes;
                children.push(group);
            }
        };
        fn refs(list: &[PlacedRef]) -> impl Iterator<Item = Node> + '_ {
            list.iter().map(|r| Node::Record(r.to_raw()))
        }
        child_group(CellChildKind::Persistent, refs(&self.persistent).collect());
        let temporary = self
            .land
            .iter()
            .map(|land| Node::Record(land.to_raw()))
            .chain(self.navmeshes.iter().map(|n| Node::Record(n.to_raw())))
            .chain(refs(&self.temporary))
            .chain(self.extra_temporary.iter().cloned().map(Node::Record))
            .collect();
        child_group(CellChildKind::Temporary, temporary);
        child_group(
            CellChildKind::VisibleDistant,
            refs(&self.visible_distant).collect(),
        );
        let mut nodes = vec![Node::Record(self.cell.to_raw())];
        if !children.children.is_empty() {
            nodes.push(Node::Group(children));
        }
        nodes
    }
}

/// A worldspace and its cells.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorldSpec {
    /// The worldspace record.
    pub world: World,
    /// The worldspace's own persistent cell, written directly in the world
    /// children group, before the blocks.
    pub persistent_cell: Option<CellSpec>,
    /// Exterior cells. Each is filed under the block and sub-block of its
    /// grid position; a cell without a grid position counts as `(0, 0)`.
    pub cells: Vec<CellSpec>,
}

impl WorldSpec {
    /// A worldspace with no cells.
    pub fn new(world: World) -> Self {
        Self {
            world,
            ..Self::default()
        }
    }

    /// Adds an exterior cell.
    #[must_use]
    pub fn with_cell(mut self, cell: CellSpec) -> Self {
        self.cells.push(cell);
        self
    }

    /// Sets the worldspace's persistent cell.
    #[must_use]
    pub fn with_persistent_cell(mut self, cell: CellSpec) -> Self {
        self.persistent_cell = Some(cell);
        self
    }

    fn nodes(&self) -> Vec<Node> {
        let world = self.world.common.meta.form_id;
        let mut children = GroupNode::world_children(world);
        if let Some(cell) = &self.persistent_cell {
            children.children.extend(cell.nodes());
        }
        // Blocks are 32 cells square and sub-blocks 8, keyed (y, x) so that
        // they are written in a fixed order.
        type Grid<T> = BTreeMap<(i16, i16), T>;
        let mut blocks: Grid<Grid<Vec<&CellSpec>>> = BTreeMap::new();
        for spec in &self.cells {
            let (x, y) = spec.cell.grid.map_or((0, 0), |g| (g.x, g.y));
            let key = |size: i32| (saturate(y.div_euclid(size)), saturate(x.div_euclid(size)));
            blocks
                .entry(key(32))
                .or_default()
                .entry(key(8))
                .or_default()
                .push(spec);
        }
        for ((block_y, block_x), sub_blocks) in &blocks {
            let mut block = GroupNode::exterior_block(*block_x, *block_y);
            for ((sub_y, sub_x), cells) in sub_blocks {
                let mut sub_block = GroupNode::exterior_sub_block(*sub_x, *sub_y);
                for spec in cells {
                    sub_block.children.extend(spec.nodes());
                }
                block.push(sub_block);
            }
            children.push(block);
        }
        let mut nodes = vec![Node::Record(self.world.to_raw())];
        if !children.children.is_empty() {
            nodes.push(Node::Group(children));
        }
        nodes
    }
}

fn saturate(value: i32) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

/// One top-level group being collected.
#[derive(Debug, Clone)]
enum Top {
    Plain(Sig, Vec<RawRecord>),
    Worlds(Vec<WorldSpec>),
    Interiors(Vec<CellSpec>),
    Topics(Vec<(Topic, Vec<Info>)>),
    Custom(Node),
}

/// Builds a plugin file in memory.
///
/// Top-level groups are written in the order their first record was added.
/// Records added with [`PluginBuilder::add`] share one group per signature;
/// worldspaces share the `WRLD` group, interior cells the `CELL` group and
/// topics the `DIAL` group. The output depends only on the calls made.
#[derive(Debug, Clone)]
pub struct PluginBuilder {
    master: bool,
    version: f32,
    author: Option<String>,
    description: Option<String>,
    masters: Vec<String>,
    next_object_id: u32,
    header_extra: Vec<RawSubrecord>,
    tops: Vec<Top>,
}

impl Default for PluginBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginBuilder {
    /// An empty plugin: no masters, no groups.
    pub fn new() -> Self {
        Self {
            master: false,
            version: PLUGIN_VERSION,
            author: None,
            description: None,
            masters: Vec::new(),
            next_object_id: 0x800,
            header_extra: Vec::new(),
            tops: Vec::new(),
        }
    }

    /// Sets or clears the master-file flag on `TES4`.
    pub fn master_file(&mut self, master: bool) -> &mut Self {
        self.master = master;
        self
    }

    /// Sets the `HEDR` version.
    pub fn version(&mut self, version: f32) -> &mut Self {
        self.version = version;
        self
    }

    /// Sets the `HEDR` next object id.
    pub fn next_object_id(&mut self, id: u32) -> &mut Self {
        self.next_object_id = id;
        self
    }

    /// Sets the author.
    pub fn author(&mut self, author: &str) -> &mut Self {
        self.author = Some(author.to_owned());
        self
    }

    /// Sets the description.
    pub fn description(&mut self, description: &str) -> &mut Self {
        self.description = Some(description.to_owned());
        self
    }

    /// Appends a master file name.
    pub fn master(&mut self, name: &str) -> &mut Self {
        self.masters.push(name.to_owned());
        self
    }

    /// Appends a subrecord to `TES4`, after the masters.
    pub fn header_subrecord(&mut self, subrecord: RawSubrecord) -> &mut Self {
        self.header_extra.push(subrecord);
        self
    }

    /// Adds a record to the top-level group of its signature.
    ///
    /// This is for record types that sit directly in a top-level group:
    /// base objects, actors, quests, scripts, textures. Cells, land,
    /// navmeshes, placed references and dialogue have their place in the
    /// nesting; add them with [`PluginBuilder::world`],
    /// [`PluginBuilder::interior`] and [`PluginBuilder::topic`].
    pub fn add(&mut self, record: impl Into<Record>) -> &mut Self {
        self.add_raw(record.into().to_raw())
    }

    /// Adds a raw record to the top-level group of its signature.
    pub fn add_raw(&mut self, record: RawRecord) -> &mut Self {
        let found = self.tops.iter_mut().find_map(|top| match top {
            Top::Plain(sig, records) if *sig == record.sig => Some(records),
            _ => None,
        });
        match found {
            Some(records) => records.push(record),
            None => self.tops.push(Top::Plain(record.sig, vec![record])),
        }
        self
    }

    /// Adds a worldspace with its cells to the `WRLD` group.
    pub fn world(&mut self, world: WorldSpec) -> &mut Self {
        let found = self.tops.iter_mut().find_map(|top| match top {
            Top::Worlds(worlds) => Some(worlds),
            _ => None,
        });
        match found {
            Some(worlds) => worlds.push(world),
            None => self.tops.push(Top::Worlds(vec![world])),
        }
        self
    }

    /// Adds an interior cell to the `CELL` group, filed under the block and
    /// sub-block of its form id (the last and the second-to-last decimal
    /// digit of the local id).
    pub fn interior(&mut self, cell: CellSpec) -> &mut Self {
        let found = self.tops.iter_mut().find_map(|top| match top {
            Top::Interiors(cells) => Some(cells),
            _ => None,
        });
        match found {
            Some(cells) => cells.push(cell),
            None => self.tops.push(Top::Interiors(vec![cell])),
        }
        self
    }

    /// Adds a topic to the `DIAL` group, followed by a topic children group
    /// holding `infos` (when there are any).
    pub fn topic(&mut self, topic: Topic, infos: Vec<Info>) -> &mut Self {
        let found = self.tops.iter_mut().find_map(|top| match top {
            Top::Topics(topics) => Some(topics),
            _ => None,
        });
        match found {
            Some(topics) => topics.push((topic, infos)),
            None => self.tops.push(Top::Topics(vec![(topic, infos)])),
        }
        self
    }

    /// Adds anything at the top level, as given: a hand-built group, or a
    /// record outside any group.
    pub fn node(&mut self, node: impl Into<Node>) -> &mut Self {
        self.tops.push(Top::Custom(node.into()));
        self
    }

    fn top_nodes(&self) -> Vec<Node> {
        self.tops
            .iter()
            .map(|top| match top {
                Top::Plain(sig, records) => {
                    let mut group = GroupNode::top(*sig);
                    group.children = records.iter().cloned().map(Node::Record).collect();
                    Node::Group(group)
                }
                Top::Worlds(worlds) => {
                    let mut group = GroupNode::top(sigs::WRLD);
                    group.children = worlds.iter().flat_map(WorldSpec::nodes).collect();
                    Node::Group(group)
                }
                Top::Interiors(cells) => Node::Group(interior_group(cells)),
                Top::Topics(topics) => {
                    let mut group = GroupNode::top(sigs::DIAL);
                    for (topic, infos) in topics {
                        group.push(topic.to_raw());
                        if !infos.is_empty() {
                            let mut children = GroupNode::topic_children(topic.common.meta.form_id);
                            children.children =
                                infos.iter().map(|i| Node::Record(i.to_raw())).collect();
                            group.push(children);
                        }
                    }
                    Node::Group(group)
                }
                Top::Custom(node) => node.clone(),
            })
            .collect()
    }

    /// Writes the plugin.
    ///
    /// # Panics
    ///
    /// When a record or group would exceed the 4 GiB a size field can hold.
    pub fn build(&self) -> Vec<u8> {
        let mut body = Vec::new();
        let mut count = 0u32;
        for node in self.top_nodes() {
            write_node(&mut body, &node, &mut count);
        }
        let mut header = RawRecord::new(b"TES4", 0).with(
            b"HEDR",
            Wr::default()
                .f32(self.version)
                .u32(count)
                .u32(self.next_object_id)
                .done(),
        );
        if self.master {
            header.meta.flags.0 |= RecordFlags::MASTER;
        }
        if let Some(author) = &self.author {
            header = header.with(b"CNAM", zstring_bytes(author));
        }
        if let Some(description) = &self.description {
            header = header.with(b"SNAM", zstring_bytes(description));
        }
        for master in &self.masters {
            header = header
                .with(b"MAST", zstring_bytes(master))
                .with(b"DATA", vec![0u8; 8]);
        }
        header.subrecords.extend(self.header_extra.iter().cloned());
        let mut out = encode_record(&header);
        out.extend_from_slice(&body);
        out
    }
}

fn interior_group(cells: &[CellSpec]) -> GroupNode {
    let mut blocks: BTreeMap<i32, BTreeMap<i32, Vec<&CellSpec>>> = BTreeMap::new();
    for spec in cells {
        let local = spec.cell.common.meta.form_id.local_id();
        let block = i32::try_from(local % 10).unwrap_or(0);
        let sub_block = i32::try_from(local / 10 % 10).unwrap_or(0);
        blocks
            .entry(block)
            .or_default()
            .entry(sub_block)
            .or_default()
            .push(spec);
    }
    let mut group = GroupNode::top(sigs::CELL);
    for (block_number, sub_blocks) in &blocks {
        let mut block = GroupNode::interior_block(*block_number);
        for (sub_number, cells) in sub_blocks {
            let mut sub_block = GroupNode::interior_sub_block(*sub_number);
            for spec in cells {
                sub_block.children.extend(spec.nodes());
            }
            block.push(sub_block);
        }
        group.push(block);
    }
    group
}

fn size_field(len: usize) -> [u8; 4] {
    u32::try_from(len)
        .expect("a fixture record or group fits a 32-bit size field")
        .to_le_bytes()
}

/// Writes subrecords as record data. A payload too long for the `u16` size
/// field is preceded by an `XXXX` subrecord holding its real size, and its
/// own size field is written as 0.
pub fn encode_subrecords(subrecords: &[RawSubrecord]) -> Vec<u8> {
    let mut out = Vec::new();
    for sub in subrecords {
        let short = u16::try_from(sub.data.len());
        if short.is_err() {
            out.extend_from_slice(b"XXXX");
            out.extend_from_slice(&4u16.to_le_bytes());
            out.extend_from_slice(&size_field(sub.data.len()));
        }
        out.extend_from_slice(&sub.sig.0);
        out.extend_from_slice(&short.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&sub.data);
    }
    out
}

/// Writes one record: the 24-byte header and the data, compressed when the
/// record's flags have [`RecordFlags::COMPRESSED`].
pub fn encode_record(record: &RawRecord) -> Vec<u8> {
    let mut data = encode_subrecords(&record.subrecords);
    if record.meta.flags.is_compressed() {
        let mut packed = size_field(data.len()).to_vec();
        let mut encoder =
            flate2::write::ZlibEncoder::new(&mut packed, flate2::Compression::default());
        encoder
            .write_all(&data)
            .and_then(|()| encoder.finish().map(|_| ()))
            .expect("compressing into a Vec cannot fail");
        data = packed;
    }
    let mut out = Vec::with_capacity(HEADER_LEN + data.len());
    out.extend_from_slice(&record.sig.0);
    out.extend_from_slice(&size_field(data.len()));
    out.extend_from_slice(&record.meta.flags.0.to_le_bytes());
    out.extend_from_slice(&record.meta.form_id.0.to_le_bytes());
    out.extend_from_slice(&record.meta.revision.to_le_bytes());
    out.extend_from_slice(&record.meta.form_version.to_le_bytes());
    out.extend_from_slice(&record.meta.unknown.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// Writes a record or a group with everything in it. `count` is advanced by
/// one per record and per group, as the `HEDR` record count wants.
pub fn write_node(out: &mut Vec<u8>, node: &Node, count: &mut u32) {
    *count = count.saturating_add(1);
    match node {
        Node::Record(record) => out.extend_from_slice(&encode_record(record)),
        Node::Group(group) => {
            let start = out.len();
            out.extend_from_slice(b"GRUP");
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&group.label);
            out.extend_from_slice(&group.group_type.to_le_bytes());
            out.extend_from_slice(&group.stamp.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            for child in &group.children {
                write_node(out, child, count);
            }
            let size = size_field(out.len() - start);
            out[start + 4..start + 8].copy_from_slice(&size);
        }
    }
}
