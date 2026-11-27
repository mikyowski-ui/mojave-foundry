//! A small synthetic Fallout 3 install: one master at `HEDR` version 0.94,
//! with a child worldspace that uses its parent's land, the older
//! subrecord sizes Fallout 3 plugins use, a terminal, a note, an AI
//! package, and references pointed at in every way the referenced set
//! knows. Everything in it is invented. It has no archives: its statics
//! have no models.

use std::fs;
use std::path::{Path, PathBuf};

use fnv_esm::fixture::{
    CellSpec, FALLOUT3_PLUGIN_VERSION, GroupNode, PluginBuilder, WorldSpec, flat_heights,
    up_normals,
};
use fnv_esm::{
    Cell, FormId, Land, Note, Package, PackageLocation, PackageTarget, PlacedKind, PlacedRef,
    Primitive, Quest, QuestObjective, QuestTarget, RawRecord, Script, ScriptBody, ScriptReference,
    Static, Terminal, TerminalMenuItem, World,
};

use crate::extract::ExtractOptions;
use crate::model::{RawStore, Slice};
use crate::paths::FormKey;

/// The plugin's file name.
pub const FALLOUT3_PLUGIN: &str = "MiniWaste.esm";
/// The parent worldspace's editor id.
pub const PARENT_WORLDSPACE: &str = "MWWasteland";
/// The child worldspace's editor id: it uses the parent's land.
pub const CHILD_WORLDSPACE: &str = "MWTownWorld";

/// What [`fallout3_town`] wrote, and the forms a test wants to look at.
#[derive(Debug, Clone, PartialEq)]
pub struct Fallout3Town {
    /// The `Data` directory.
    pub data_dir: PathBuf,
    /// The plugin list: [`FALLOUT3_PLUGIN`].
    pub plugins: Vec<String>,
    /// The child worldspace's cells x 0 to 2, y 0, no interiors. Cell
    /// (0, 0) has no land of its own, (1, 0) has, and (2, 0) has no cell
    /// record (a persistent reference stands in it).
    pub slice: Slice,
    /// The parent's land at (0, 0), which the child's cell (0, 0) uses.
    pub parent_land: FormKey,
    /// The child's own land at (1, 0).
    pub child_land: FormKey,
    /// The child's cell (0, 0).
    pub child_cell: FormKey,
    /// A persistent reference standing in (2, 0), where the child has no
    /// cell record.
    pub gridless_ref: FormKey,
    /// A persistent reference standing in (1, 0).
    pub persistent_ref: FormKey,
    /// The target of a quest objective.
    pub quest_target: FormKey,
    /// A reference a script record names (`SCRO`).
    pub script_target: FormKey,
    /// The reference a dialogue condition runs on.
    pub condition_target: FormKey,
    /// A reference with a linked reference ...
    pub link_source: FormKey,
    /// ... and that linked reference.
    pub link_target: FormKey,
    /// An enable parent ...
    pub enable_parent: FormKey,
    /// ... and its child.
    pub enable_child: FormKey,
    /// The package's target reference.
    pub package_target: FormKey,
    /// The package's location reference.
    pub package_location: FormKey,
    /// A reference a terminal item's script names.
    pub terminal_target: FormKey,
    /// A reference nothing points at.
    pub plain_ref: FormKey,
    /// The settler's reference; nothing points at it either.
    pub settler_ref: FormKey,
    /// A trigger volume in cell (0, 0): a reference to an activator
    /// (`MWTrigger`, no model) with a sphere primitive of radius 192.
    pub trigger_volume: FormKey,
    /// An NPC whose `DATA` is the older 25-byte form (skills first,
    /// health 75), and whose one package is [`Self::package`].
    pub settler: FormKey,
    /// A sandbox package.
    pub package: FormKey,
    /// A terminal with three menu items.
    pub terminal: FormKey,
    /// A text note, shown by the terminal's second item.
    pub note: FormKey,
    /// A quest with one objective target.
    pub quest: FormKey,
    /// A script naming [`Self::script_target`].
    pub script: FormKey,
    /// A dialogue response group with the 3-byte `DATA` (flags 5) and a
    /// 16-byte `TRDT`.
    pub info: FormKey,
    /// A weapon with a 136-byte `DNAM` (skill 33, action points 30).
    pub weapon: FormKey,
}

impl Fallout3Town {
    /// Extract options for this install.
    pub fn options(&self, out: &Path, jobs: usize) -> ExtractOptions {
        ExtractOptions {
            data_dir: self.data_dir.clone(),
            plugins: self.plugins.clone(),
            out: out.to_path_buf(),
            slice: self.slice.clone(),
            jobs,
            raw_store: RawStore::All,
        }
    }
}

fn key(id: u32) -> FormKey {
    FormKey::new(FALLOUT3_PLUGIN, id)
}

fn land(id: u32, height: f32) -> Land {
    let mut land = Land::new(id).with_heights(&flat_heights(height));
    land.flags = Some(1);
    land.normals = Some(up_normals());
    land
}

/// A `CTDA` that runs function 72 on a reference (run-on 2).
fn condition_on(reference: u32) -> Vec<u8> {
    let mut data = vec![0u8; 28];
    data[4..8].copy_from_slice(&1.0f32.to_le_bytes());
    data[8..12].copy_from_slice(&72u32.to_le_bytes());
    data[20..24].copy_from_slice(&2u32.to_le_bytes());
    data[24..28].copy_from_slice(&reference.to_le_bytes());
    data
}

fn zstr(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

/// Writes the install under `dir` and returns what it holds.
///
/// # Panics
///
/// When the files cannot be written.
pub fn fallout3_town(dir: &Path) -> Fallout3Town {
    let data = dir.join("Data");
    fs::create_dir_all(&data).expect("create the data directory");

    const PARENT: u32 = 0x900;
    const CHILD: u32 = 0x901;
    const PARENT_CELL0: u32 = 0xA00;
    const PARENT_CELL1: u32 = 0xA01;
    const PARENT_LAND0: u32 = 0xA10;
    const PARENT_LAND1: u32 = 0xA11;
    const CHILD_CELL0: u32 = 0xB00;
    const CHILD_CELL1: u32 = 0xB01;
    const CHILD_PERSISTENT: u32 = 0xB0F;
    const CHILD_LAND1: u32 = 0xB11;
    const CRATE: u32 = 0x800;
    const SETTLER: u32 = 0x810;
    const PACKAGE: u32 = 0x850;
    const SCRIPT: u32 = 0x860;
    const QUEST: u32 = 0x870;
    const TOPIC: u32 = 0x880;
    const INFO: u32 = 0x881;
    const TERMINAL: u32 = 0x890;
    const NOTE: u32 = 0x8A0;
    const WEAPON: u32 = 0x8B0;
    const R_QUEST: u32 = 0xC00;
    const R_SCRIPT: u32 = 0xC01;
    const R_CONDITION: u32 = 0xC02;
    const R_LINK_SOURCE: u32 = 0xC03;
    const R_LINK_TARGET: u32 = 0xC04;
    const R_PARENT: u32 = 0xC05;
    const R_CHILD: u32 = 0xC06;
    const R_PACKAGE_TARGET: u32 = 0xC07;
    const R_PACKAGE_LOCATION: u32 = 0xC08;
    const R_TERMINAL: u32 = 0xC09;
    const R_PLAIN: u32 = 0xC0A;
    const R_SETTLER: u32 = 0xC0B;
    const R_TRIGGER: u32 = 0xC0C;
    const TRIGGER: u32 = 0x8C0;
    const R_GRIDLESS: u32 = 0xC20;
    const R_PERSISTENT: u32 = 0xC21;

    let mut plugin = PluginBuilder::new();
    plugin
        .version(FALLOUT3_PLUGIN_VERSION)
        .master_file(true)
        .author("mojave-foundry fixture");

    plugin.add(Static::new(CRATE, "MWCrate", ""));
    plugin.add_raw(RawRecord::new(b"ACTI", TRIGGER).with(b"EDID", zstr("MWTrigger")));

    // An NPC with the older 25-byte DATA: 14 skill values, health, the
    // seven attributes.
    let mut npc_data: Vec<u8> = vec![15; 14];
    npc_data.extend_from_slice(&75i32.to_le_bytes());
    npc_data.extend_from_slice(&[5, 6, 4, 7, 5, 6, 3]);
    plugin.add_raw(
        RawRecord::new(b"NPC_", SETTLER)
            .with(b"EDID", zstr("MWSettler"))
            .with(b"FULL", zstr("Settler"))
            .with(b"ACBS", vec![0; 24])
            .with(b"PKID", PACKAGE.to_le_bytes())
            .with(b"DATA", npc_data),
    );

    let mut package = Package::new(PACKAGE, "MWSettlerSandbox", 12);
    package.location = Some(PackageLocation {
        location_type: PackageLocation::NEAR_REFERENCE,
        value: R_PACKAGE_LOCATION,
        radius: 256,
    });
    package.target = Some(PackageTarget {
        target_type: PackageTarget::SPECIFIC_REFERENCE,
        value: R_PACKAGE_TARGET,
        count: 0,
        extra: Some(0.0),
    });
    package.on_begin = Some(fnv_esm::PackageEvent {
        idle: Some(FormId::NULL),
        script: ScriptBody::from_source("SetStage MWQuest 10"),
        topic: Some(FormId::NULL),
    });
    plugin.add(package);

    let mut script = Script::new(SCRIPT, "MWScript", "ref crate\r\nbegin GameMode\r\nend\r\n");
    script.body.references = vec![ScriptReference::Form(FormId(R_SCRIPT))];
    plugin.add(script);

    plugin.add(Quest {
        objectives: vec![QuestObjective {
            index: 10,
            description: Some("Find the crate.".to_owned()),
            targets: vec![QuestTarget {
                target: FormId(R_QUEST),
                flags: 0,
                conditions: Vec::new(),
            }],
        }],
        ..Quest::new(QUEST, "MWQuest", "A Crate")
    });

    // A topic with one info in the older forms: a 3-byte DATA and a
    // 16-byte TRDT.
    let mut trdt = vec![0u8; 16];
    trdt[..4].copy_from_slice(&5u32.to_le_bytes());
    trdt[4..8].copy_from_slice(&50i32.to_le_bytes());
    trdt[12] = 1;
    let topic = RawRecord::new(b"DIAL", TOPIC)
        .with(b"EDID", zstr("MWGreeting"))
        .with(b"QSTI", QUEST.to_le_bytes())
        .with(b"FULL", zstr("Hello"))
        .with(b"DATA", vec![0, 0]);
    let info = RawRecord::new(b"INFO", INFO)
        .with(b"DATA", vec![0, 1, 5])
        .with(b"QSTI", QUEST.to_le_bytes())
        .with(b"TRDT", trdt)
        .with(b"NAM1", zstr("Hello there."))
        .with(b"CTDA", condition_on(R_CONDITION));
    plugin.node(
        GroupNode::top(fnv_esm::Sig::new(b"DIAL"))
            .with(topic)
            .with(GroupNode::topic_children(FormId(TOPIC)).with(info)),
    );

    let mut opening = TerminalMenuItem::new("Open the crate", "Opened.");
    opening.script = ScriptBody {
        references: vec![ScriptReference::Form(FormId(R_TERMINAL))],
        ..ScriptBody::from_source("crate.Activate player")
    };
    let mut reading = TerminalMenuItem::new("Read the log", "");
    reading.display_note = Some(FormId(NOTE));
    let mut locked = TerminalMenuItem::new("Maintenance", "Access denied.");
    locked.conditions = vec![fnv_esm::Condition::float(
        14,
        fnv_esm::Comparison::Greater,
        50.0,
    )];
    plugin.add(Terminal {
        items: vec![opening, reading, locked],
        ..Terminal::new(TERMINAL, "MWTerminal", "Overseer", "WELCOME")
    });
    plugin.add(Note::text(NOTE, "MWLog", "Log", "Day one."));

    let mut dnam = vec![0u8; 136];
    dnam[68..72].copy_from_slice(&30.0f32.to_le_bytes());
    dnam[104..108].copy_from_slice(&33u32.to_le_bytes());
    plugin.add_raw(
        RawRecord::new(b"WEAP", WEAPON)
            .with(b"EDID", zstr("MWPistol"))
            .with(b"DNAM", dnam),
    );

    // The parent worldspace: land at (0, 0) and (1, 0).
    let parent = WorldSpec::new(World::new(PARENT, PARENT_WORLDSPACE))
        .with_cell(
            CellSpec::new(Cell::exterior(PARENT_CELL0, 0, 0)).with_land(land(PARENT_LAND0, 100.0)),
        )
        .with_cell(
            CellSpec::new(Cell::exterior(PARENT_CELL1, 1, 0)).with_land(land(PARENT_LAND1, 100.0)),
        );
    plugin.world(parent);

    // The child: it uses the parent's land (PNAM bit 0, as Fallout 3's
    // town worldspaces store it), has no land at (0, 0) and no cell at
    // (2, 0).
    let mut child_world = World::new(CHILD, CHILD_WORLDSPACE);
    child_world.parent = Some(FormId(PARENT));
    child_world.parent_flags = Some(0xFFC5);
    let at = |i: u32| [100.0 + 200.0 * i as f32, 100.0, 100.0];
    let object =
        |form: u32, i: u32| PlacedRef::new(PlacedKind::Object, form, CRATE, at(i), [0.0; 3]);
    let mut link_source = object(R_LINK_SOURCE, 3);
    link_source.linked_ref = Some(FormId(R_LINK_TARGET));
    let mut cell0 = CellSpec::new(Cell::exterior(CHILD_CELL0, 0, 0));
    cell0.temporary = vec![
        object(R_QUEST, 0),
        object(R_SCRIPT, 1),
        object(R_CONDITION, 2),
        link_source,
        object(R_LINK_TARGET, 4),
        object(R_PARENT, 5),
        object(R_CHILD, 6).with_enable_parent(R_PARENT, false),
        object(R_PACKAGE_TARGET, 7),
        object(R_PACKAGE_LOCATION, 8),
        object(R_TERMINAL, 9),
        object(R_PLAIN, 10),
        PlacedRef::new(PlacedKind::Npc, R_SETTLER, SETTLER, at(11), [0.0; 3]),
        PlacedRef::new(PlacedKind::Object, R_TRIGGER, TRIGGER, at(12), [0.0; 3]).with_primitive(
            Primitive {
                bounds: [192.0, 192.0, 192.0],
                color: [0.2, 0.6, 0.9],
                unknown: 0.5,
                shape: Primitive::SHAPE_SPHERE,
            },
        ),
    ];
    let cell1 =
        CellSpec::new(Cell::exterior(CHILD_CELL1, 1, 0)).with_land(land(CHILD_LAND1, 300.0));
    let mut persistent = CellSpec::new(Cell::exterior(CHILD_PERSISTENT, 0, 0));
    persistent.persistent = vec![
        PlacedRef::new(
            PlacedKind::Object,
            R_GRIDLESS,
            CRATE,
            [2.0 * 4096.0 + 500.0, 500.0, 0.0],
            [0.0; 3],
        )
        .persistent(),
        PlacedRef::new(
            PlacedKind::Object,
            R_PERSISTENT,
            CRATE,
            [4096.0 + 500.0, 500.0, 0.0],
            [0.0; 3],
        )
        .persistent(),
    ];
    plugin.world(
        WorldSpec::new(child_world)
            .with_persistent_cell(persistent)
            .with_cell(cell0)
            .with_cell(cell1),
    );

    fs::write(data.join(FALLOUT3_PLUGIN), plugin.build()).expect("write the plugin");

    Fallout3Town {
        data_dir: data,
        plugins: vec![FALLOUT3_PLUGIN.to_owned()],
        slice: Slice {
            name: "minitown3".to_owned(),
            worldspace: CHILD_WORLDSPACE.to_owned(),
            x: 0..=2,
            y: 0..=0,
            interior_depth: 0,
        },
        parent_land: key(PARENT_LAND0),
        child_land: key(CHILD_LAND1),
        child_cell: key(CHILD_CELL0),
        gridless_ref: key(R_GRIDLESS),
        persistent_ref: key(R_PERSISTENT),
        quest_target: key(R_QUEST),
        script_target: key(R_SCRIPT),
        condition_target: key(R_CONDITION),
        link_source: key(R_LINK_SOURCE),
        link_target: key(R_LINK_TARGET),
        enable_parent: key(R_PARENT),
        enable_child: key(R_CHILD),
        package_target: key(R_PACKAGE_TARGET),
        package_location: key(R_PACKAGE_LOCATION),
        terminal_target: key(R_TERMINAL),
        plain_ref: key(R_PLAIN),
        trigger_volume: key(R_TRIGGER),
        settler_ref: key(R_SETTLER),
        settler: key(SETTLER),
        package: key(PACKAGE),
        terminal: key(TERMINAL),
        note: key(NOTE),
        quest: key(QUEST),
        script: key(SCRIPT),
        info: key(INFO),
        weapon: key(WEAPON),
    }
}
