//! The walker: header, group nesting, record contexts, skipping,
//! compression, the XXXX escape, and preservation of unknown data.

use fnv_esm::fixture::{
    CellSpec, GroupNode, PluginBuilder, WorldSpec, encode_subrecords, flat_heights,
};
use fnv_esm::*;

const WORLD: u32 = 0x0100_2000;
const WORLD_CELL: u32 = 0x0100_2001;
const CELL_NEAR: u32 = 0x0100_2010;
const CELL_FAR: u32 = 0x0100_2020;
const CELL_NEGATIVE: u32 = 0x0100_2030;
const INTERIOR_A: u32 = 0x0100_3047;
const INTERIOR_B: u32 = 0x0100_3052;

fn reference(form_id: u32) -> PlacedRef {
    PlacedRef::new(PlacedKind::Object, form_id, 0x0100_1000, [0.0; 3], [0.0; 3])
}

fn town() -> Vec<u8> {
    let mut builder = PluginBuilder::new();
    builder
        .master_file(true)
        .author("walker test")
        .description("A synthetic plugin.")
        .master("Base.esm")
        .next_object_id(0x4000);
    builder.add(Static::new(0x0100_1000, "TestCrate", "clutter\\crate.nif"));
    builder.add(Static::new(
        0x0100_1001,
        "TestBarrel",
        "clutter\\barrel.nif",
    ));
    builder.add(Door::new(
        0x0100_1100,
        "TestDoor",
        "Door",
        "clutter\\door.nif",
    ));
    builder.world(
        WorldSpec::new(World::new(WORLD, "TestWorld"))
            .with_persistent_cell(
                CellSpec::new(Cell::exterior(WORLD_CELL, 0, 0))
                    .with_persistent(reference(0x0100_2002).persistent()),
            )
            .with_cell(
                CellSpec::new(Cell::exterior(CELL_NEAR, 3, -2))
                    .with_land(Land::new(0x0100_2011).with_heights(&flat_heights(256.0)))
                    .with_navmesh(Navmesh::new(0x0100_2012, CELL_NEAR, Vec::new(), Vec::new()))
                    .with_persistent(reference(0x0100_2013).persistent())
                    .with_temporary(reference(0x0100_2014))
                    .with_visible_distant(reference(0x0100_2015)),
            )
            .with_cell(
                CellSpec::new(Cell::exterior(CELL_FAR, 40, 9))
                    .with_temporary(reference(0x0100_2021)),
            )
            .with_cell(CellSpec::new(Cell::exterior(CELL_NEGATIVE, -1, -33))),
    );
    builder.interior(
        CellSpec::new(Cell::interior(INTERIOR_A, "TestShop"))
            .with_temporary(reference(0x0100_3048)),
    );
    builder.interior(CellSpec::new(Cell::interior(INTERIOR_B, "TestCellar")));
    builder.topic(
        Topic::new(0x0100_4000, "TestGreeting", "Hello", 0x0100_4100u32),
        vec![Info::new(
            0x0100_4001,
            0x0100_4100u32,
            vec![Response::new(1, "Welcome.")],
        )],
    );
    builder.build()
}

/// What the tests check about one record's place in the file.
#[derive(Debug, PartialEq)]
struct Place {
    sig: String,
    form: u32,
    top: String,
    depth: usize,
    world: Option<u32>,
    cell: Option<u32>,
    kind: Option<CellChildKind>,
}

fn places(bytes: &[u8]) -> Vec<Place> {
    let plugin = Plugin::parse(bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut out = Vec::new();
    while let Some(record) = walker.next_record().expect("walk") {
        let context = walker.context();
        out.push(Place {
            sig: record.sig().to_string(),
            form: record.form_id().0,
            top: context.top().map(|s| s.to_string()).unwrap_or_default(),
            depth: context.depth(),
            world: context.worldspace().map(|f| f.0),
            cell: context.cell().map(|f| f.0),
            kind: context.cell_child_kind(),
        });
    }
    out
}

fn place(places: &[Place], form: u32) -> &Place {
    places
        .iter()
        .find(|p| p.form == form)
        .unwrap_or_else(|| panic!("no record {form:08X}"))
}

#[test]
fn header_fields_are_exposed() {
    let bytes = town();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let header = plugin.header();
    assert!(header.is_master());
    assert_eq!(header.version, 1.34);
    assert_eq!(header.next_object_id, 0x4000);
    assert_eq!(header.author.as_deref(), Some("walker test"));
    assert_eq!(header.description.as_deref(), Some("A synthetic plugin."));
    assert_eq!(header.masters, ["Base.esm"]);
    assert!(header.unknown.is_empty());

    // The record count is the number of records and groups after TES4.
    let mut walker = plugin.walk();
    let mut count = 0u32;
    while let Some(event) = walker.next_event().expect("walk") {
        if !matches!(event, Event::GroupEnd(_)) {
            count += 1;
        }
    }
    assert_eq!(header.record_count, count);
}

#[test]
fn an_empty_plugin_has_a_header_and_nothing_else() {
    let bytes = PluginBuilder::new().build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    assert!(!plugin.header().is_master());
    assert_eq!(plugin.header().record_count, 0);
    assert_eq!(plugin.header().author, None);
    assert!(plugin.header().masters.is_empty());
    assert!(plugin.walk().next_event().expect("walk").is_none());
    assert_eq!(plugin.top_level().count(), 0);
}

#[test]
fn unknown_header_subrecords_are_kept() {
    let mut builder = PluginBuilder::new();
    builder
        .master("A.esm")
        .header_subrecord(RawSubrecord::new(b"ONAM", [1, 2, 3, 4]))
        // A DATA that is not the 8 zero bytes after a master is kept too.
        .header_subrecord(RawSubrecord::new(b"DATA", [9, 9]));
    let bytes = builder.build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    assert_eq!(plugin.header().masters, ["A.esm"]);
    assert_eq!(
        plugin.header().unknown,
        [
            RawSubrecord::new(b"ONAM", [1, 2, 3, 4]),
            RawSubrecord::new(b"DATA", [9, 9])
        ]
    );
}

#[test]
fn records_know_their_worldspace_cell_and_children_group() {
    let places = places(&town());

    // Base objects sit one group deep.
    let crate_ = place(&places, 0x0100_1000);
    assert_eq!((crate_.top.as_str(), crate_.depth), ("STAT", 1));
    assert_eq!((crate_.world, crate_.cell, crate_.kind), (None, None, None));

    // The worldspace record is in the top group; everything under it is in
    // its world children group.
    let world = place(&places, WORLD);
    assert_eq!(
        (world.top.as_str(), world.depth, world.world),
        ("WRLD", 1, None)
    );

    // The worldspace's persistent cell sits directly in world children.
    let world_cell = place(&places, WORLD_CELL);
    assert_eq!(
        (world_cell.depth, world_cell.world, world_cell.cell),
        (2, Some(WORLD), None)
    );
    let world_ref = place(&places, 0x0100_2002);
    assert_eq!(world_ref.depth, 4);
    assert_eq!(world_ref.cell, Some(WORLD_CELL));
    assert_eq!(world_ref.kind, Some(CellChildKind::Persistent));

    // An exterior cell: top, world children, block, sub-block.
    let cell = place(&places, CELL_NEAR);
    assert_eq!((cell.sig.as_str(), cell.depth), ("CELL", 4));
    assert_eq!(
        (cell.world, cell.cell, cell.kind),
        (Some(WORLD), None, None)
    );

    // Its children: two more groups deep.
    for (form, sig, kind) in [
        (0x0100_2011, "LAND", CellChildKind::Temporary),
        (0x0100_2012, "NAVM", CellChildKind::Temporary),
        (0x0100_2013, "REFR", CellChildKind::Persistent),
        (0x0100_2014, "REFR", CellChildKind::Temporary),
        (0x0100_2015, "REFR", CellChildKind::VisibleDistant),
    ] {
        let p = place(&places, form);
        assert_eq!(p.sig, sig);
        assert_eq!(p.depth, 6);
        assert_eq!(p.top, "WRLD");
        assert_eq!(p.world, Some(WORLD));
        assert_eq!(p.cell, Some(CELL_NEAR));
        assert_eq!(p.kind, Some(kind));
    }

    // Interior cells: top, block, sub-block; no worldspace.
    let shop = place(&places, INTERIOR_A);
    assert_eq!(
        (shop.top.as_str(), shop.depth, shop.world),
        ("CELL", 3, None)
    );
    let shop_ref = place(&places, 0x0100_3048);
    assert_eq!(shop_ref.depth, 5);
    assert_eq!((shop_ref.world, shop_ref.cell), (None, Some(INTERIOR_A)));
    assert_eq!(shop_ref.kind, Some(CellChildKind::Temporary));
}

#[test]
fn blocks_and_sub_blocks_follow_the_grid_and_the_form_id() {
    let bytes = town();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut exterior = Vec::new();
    let mut interior = Vec::new();
    let mut topics = Vec::new();
    while let Some(record) = walker.next_record().expect("walk") {
        let context = walker.context();
        if record.sig() == sigs::CELL && context.worldspace().is_some() {
            exterior.push((
                record.form_id().0,
                context.exterior_block(),
                context.exterior_sub_block(),
            ));
        } else if record.sig() == sigs::CELL {
            interior.push((
                record.form_id().0,
                context.interior_block(),
                context.interior_sub_block(),
            ));
        } else if record.sig() == sigs::INFO {
            topics.push((
                record.form_id().0,
                context.topic().map(|f| f.0),
                context.depth(),
            ));
        }
    }
    // Blocks are 32 cells, sub-blocks 8, both rounded toward minus infinity;
    // the pairs are (x, y). Blocks are written in (y, x) order.
    assert_eq!(
        exterior,
        [
            (WORLD_CELL, None, None),
            (CELL_NEGATIVE, Some((-1, -2)), Some((-1, -5))),
            (CELL_NEAR, Some((0, -1)), Some((0, -1))),
            (CELL_FAR, Some((1, 0)), Some((5, 1))),
        ]
    );
    // Interior block: last decimal digit of the local id; sub-block: the
    // digit before it. 0x3047 = 12359, 0x3052 = 12370.
    assert_eq!(
        interior,
        [
            (INTERIOR_B, Some(0), Some(7)),
            (INTERIOR_A, Some(9), Some(5)),
        ]
    );
    assert_eq!(topics, [(0x0100_4001, Some(0x0100_4000), 2)]);
}

#[test]
fn group_events_nest_and_carry_the_chain() {
    let bytes = town();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut depth = 0usize;
    let mut deepest = 0usize;
    let mut saw_chain = false;
    while let Some(event) = walker.next_event().expect("walk") {
        match event {
            Event::GroupStart(header) => {
                depth += 1;
                deepest = deepest.max(depth);
                assert_eq!(walker.context().depth(), depth);
                assert_eq!(walker.context().chain().last(), Some(&header));
                assert_eq!(&bytes[header.offset..header.offset + 4], b"GRUP");
            }
            Event::GroupEnd(header) => {
                depth -= 1;
                assert_eq!(walker.context().depth(), depth);
                assert_eq!(walker.offset(), header.offset + header.size as usize);
            }
            Event::Record(record) if record.form_id().0 == 0x0100_2013 => {
                let kinds: Vec<GroupKind> = walker
                    .context()
                    .chain()
                    .iter()
                    .map(GroupHeader::kind)
                    .collect();
                assert_eq!(
                    kinds,
                    [
                        GroupKind::Top(sigs::WRLD),
                        GroupKind::WorldChildren(FormId(WORLD)),
                        GroupKind::ExteriorBlock { x: 0, y: -1 },
                        GroupKind::ExteriorSubBlock { x: 0, y: -1 },
                        GroupKind::CellChildren(FormId(CELL_NEAR)),
                        GroupKind::CellChild(CellChildKind::Persistent, FormId(CELL_NEAR)),
                    ]
                );
                saw_chain = true;
            }
            Event::Record(record) => {
                assert_eq!(
                    &bytes[record.offset..record.offset + 4],
                    record.sig().as_bytes()
                );
            }
        }
    }
    assert_eq!(depth, 0);
    assert_eq!(deepest, 6);
    assert!(saw_chain);
    // The walk is over: it stays over.
    assert!(walker.next_event().expect("walk").is_none());
}

#[test]
fn a_top_level_group_is_skipped_without_reading_it() {
    let mut bytes = town();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let labels: Vec<String> = plugin
        .top_level()
        .map(|item| match item.expect("top level") {
            TopItem::Group(group) => match group.header.kind() {
                GroupKind::Top(sig) => sig.to_string(),
                other => panic!("unexpected top-level group {other:?}"),
            },
            TopItem::Record(record) => panic!("stray record {}", record.sig()),
        })
        .collect();
    assert_eq!(labels, ["STAT", "DOOR", "WRLD", "CELL", "DIAL"]);

    // Walking one group gives its records with the group in their context.
    let statics = plugin
        .top_group(sigs::STAT)
        .expect("top level")
        .expect("STAT group");
    let mut walker = statics.walk();
    let mut forms = Vec::new();
    while let Some(record) = walker.next_record().expect("walk") {
        assert_eq!(walker.context().top(), Some(sigs::STAT));
        assert_eq!(walker.context().depth(), 1);
        forms.push(record.form_id().0);
    }
    assert_eq!(forms, [0x0100_1000, 0x0100_1001]);
    assert_eq!(walker.context().depth(), 1);
    assert!(
        plugin
            .top_group(Sig::new(b"GMST"))
            .expect("top level")
            .is_none()
    );

    // Skipping from the walker: the group's start and end, nothing between.
    let mut walker = plugin.walk();
    let mut seen = Vec::new();
    while let Some(event) = walker.next_event().expect("walk") {
        match event {
            Event::GroupStart(header) => {
                if header.kind() == GroupKind::Top(sigs::WRLD) {
                    walker.skip_group();
                }
            }
            Event::GroupEnd(_) => {}
            Event::Record(record) => seen.push(record.sig().to_string()),
        }
    }
    assert!(!seen.is_empty());
    assert!(
        !seen
            .iter()
            .any(|s| s == "WRLD" || s == "LAND" || s == "NAVM")
    );
    assert!(
        seen.iter().any(|s| s == "CELL"),
        "interior cells are still walked"
    );

    // Proof that skipped bytes are never read: wreck the inside of the WRLD
    // group and the top-level pass and the skipping walk still succeed.
    let world_group = plugin
        .top_group(sigs::WRLD)
        .expect("top level")
        .expect("WRLD group");
    let start = world_group.header.offset + HEADER_LEN;
    let end = world_group.header.offset + world_group.header.size as usize;
    assert_eq!(world_group.contents().len(), end - start);
    bytes[start..end].fill(0xFF);
    let plugin = Plugin::parse(&bytes).expect("plugin");
    assert_eq!(plugin.top_level().filter(Result::is_ok).count(), 5);
    let mut walker = plugin.walk();
    let mut records = 0;
    while let Some(event) = walker.next_event().expect("skipping walk") {
        match event {
            Event::GroupStart(header) if header.kind() == GroupKind::Top(sigs::WRLD) => {
                walker.skip_group();
            }
            Event::Record(_) => records += 1,
            _ => {}
        }
    }
    assert_eq!(records, seen.len());
    // Without the skip, the wreckage is an error, not a panic.
    assert!(plugin.walk().any(|event| event.is_err()));
}

#[test]
fn a_compressed_record_is_inflated_only_on_demand() {
    let text = "begin GameMode\r\n\t; nothing yet\r\nend\r\n".repeat(40);
    let mut script = Script::new(0x0100_5000, "TestLongScript", &text);
    script.common = script.common.with_flags(RecordFlags::COMPRESSED);
    let plain = Script::new(0x0100_5001, "TestShortScript", "begin GameMode\r\nend\r\n");
    let mut builder = PluginBuilder::new();
    builder.add(script.clone()).add(plain.clone());
    let bytes = builder.build();

    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let packed = walker.next_record().expect("walk").expect("first record");
    let stored = walker.next_record().expect("walk").expect("second record");

    assert!(packed.flags().is_compressed());
    let uncompressed_len = encode_subrecords(&script.to_raw().subrecords).len();
    assert!((packed.header.data_size as usize) < uncompressed_len / 2);
    assert_eq!(
        u32::from_le_bytes(packed.stored_data()[..4].try_into().expect("4 bytes")) as usize,
        uncompressed_len
    );
    let data = packed.data().expect("inflate");
    assert!(data.was_compressed());
    assert_eq!(data.bytes().len(), uncompressed_len);
    // Subrecord offsets inside a compressed record are the record's offset.
    assert!(
        data.subrecords()
            .all(|s| s.expect("subrecord").offset == packed.offset)
    );
    assert_eq!(packed.decode().expect("decode"), Record::Script(script));

    // An uncompressed record borrows from the file and reports file offsets.
    assert!(!stored.flags().is_compressed());
    let data = stored.data().expect("data");
    assert!(!data.was_compressed());
    assert_eq!(data.bytes().as_ptr(), stored.stored_data().as_ptr());
    let first = data
        .subrecords()
        .next()
        .expect("one subrecord")
        .expect("subrecord");
    assert_eq!(first.offset, stored.offset + HEADER_LEN);
    assert_eq!(&bytes[first.offset..first.offset + 4], b"EDID");
    assert_eq!(stored.decode().expect("decode"), Record::Script(plain));

    // The cap is the caller's to set.
    assert!(matches!(
        packed.data_with_limit(uncompressed_len - 1),
        Err(EsmError::DecompressedTooLarge { .. })
    ));
    assert!(packed.data_with_limit(uncompressed_len).is_ok());
}

#[test]
fn a_large_subrecord_uses_the_extended_size_escape() {
    let big: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    let exact: Vec<u8> = vec![0xAB; 65_536];
    let edge: Vec<u8> = vec![0xCD; 65_535];
    let raw = RawRecord::new(b"NAVM", 0x0100_6000u32)
        .with(b"EDID", &b"TestBigNavmesh\0"[..])
        .with(b"NVGD", big.clone())
        .with(b"ZZZ1", exact.clone())
        .with(b"ZZZ2", edge.clone())
        .with(b"NVER", 11u32.to_le_bytes());

    // 65 535 bytes still fit the u16; 65 536 and more need XXXX.
    let encoded = encode_subrecords(&raw.subrecords);
    let markers = encoded.windows(6).filter(|w| *w == b"XXXX\x04\x00").count();
    assert_eq!(markers, 2);
    let at = encoded
        .windows(4)
        .position(|w| w == b"XXXX")
        .expect("marker");
    assert_eq!(&encoded[at + 6..at + 10], &70_000u32.to_le_bytes());
    assert_eq!(&encoded[at + 10..at + 16], b"NVGD\x00\x00");

    for compressed in [false, true] {
        let mut record = raw.clone();
        if compressed {
            record.meta.flags.0 |= RecordFlags::COMPRESSED;
        }
        let mut builder = PluginBuilder::new();
        builder.add_raw(record.clone());
        let bytes = builder.build();
        let plugin = Plugin::parse(&bytes).expect("plugin");
        let stored = plugin.walk().next_record().expect("walk").expect("record");
        let data = stored.data().expect("data");
        let subs = data.collect().expect("subrecords");
        let sigs: Vec<String> = subs.iter().map(|s| s.sig.to_string()).collect();
        assert_eq!(sigs, ["EDID", "NVGD", "ZZZ1", "ZZZ2", "NVER"]);
        assert_eq!(subs[1].data, big.as_slice());
        assert_eq!(subs[2].data.len(), 65_536);
        assert_eq!(subs[3].data.len(), 65_535);
        assert_eq!(stored.to_raw().expect("raw"), record);

        // The typed decoder sees through the escape too.
        let Record::Navmesh(navmesh) = stored.decode().expect("decode") else {
            panic!("NAVM decodes to a navmesh");
        };
        assert_eq!(navmesh.version, Some(11));
        assert_eq!(navmesh.common.editor_id.as_deref(), Some("TestBigNavmesh"));
        assert_eq!(navmesh.unknown.len(), 3);
        assert_eq!(navmesh.unknown[0].data, big);
    }
}

#[test]
fn unknown_records_and_subrecords_are_preserved() {
    // A record type with no decoder.
    let gadget = RawRecord::new(b"GDGT", 0x0100_7000u32)
        .with(b"EDID", &b"TestGadget\0"[..])
        .with(b"QQQQ", vec![1, 2, 3])
        .with(b"QQQQ", Vec::new());
    // A typed record with subrecords its decoder does not know, a repeat of
    // one it takes once, and one of a size it has no layout for.
    let odd_static = RawRecord::new(b"STAT", 0x0100_7001u32)
        .with(b"EDID", &b"TestOddStatic\0"[..])
        .with(b"WHAT", vec![0xFE; 5])
        .with(b"MODL", &b"clutter\\odd.nif\0"[..])
        .with(b"MODL", &b"clutter\\second.nif\0"[..])
        .with(b"OBND", vec![0; 7])
        .with(b"MODT", vec![0x11; 24]);
    let odd_ref = RawRecord::new(b"REFR", 0x0100_7002u32)
        .with(b"NAME", 0x0100_7001u32.to_le_bytes())
        .with(b"DATA", vec![0; 23])
        .with(b"XSCL", vec![0; 3])
        .with(b"XTEL", vec![0; 30])
        .with(b"XESP", vec![0; 9]);

    let mut builder = PluginBuilder::new();
    builder
        .add_raw(gadget.clone())
        .add_raw(odd_static.clone())
        .node(GroupNode::new(77, *b"odd!").with(odd_ref.clone()));
    let bytes = builder.build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();

    let record = walker.next_record().expect("walk").expect("gadget");
    assert_eq!(
        record.decode().expect("decode"),
        Record::Unknown(gadget.clone())
    );
    assert_eq!(record.to_raw().expect("raw"), gadget);
    let unknown = record.decode().expect("decode");
    assert_eq!(unknown.sig(), Sig::new(b"GDGT"));
    assert!(unknown.common().is_none());
    assert_eq!(unknown.unknown_subrecords().len(), 3);

    let record = walker.next_record().expect("walk").expect("static");
    let Record::Static(decoded) = record.decode().expect("decode") else {
        panic!("STAT decodes to a static");
    };
    assert_eq!(decoded.common.editor_id.as_deref(), Some("TestOddStatic"));
    assert_eq!(decoded.common.model.as_deref(), Some("clutter\\odd.nif"));
    assert_eq!(decoded.common.bounds, None);
    let kept: Vec<(String, usize)> = decoded
        .unknown
        .iter()
        .map(|s| (s.sig.to_string(), s.data.len()))
        .collect();
    assert_eq!(
        kept,
        [
            ("WHAT".to_owned(), 5),
            ("MODL".to_owned(), 19),
            ("OBND".to_owned(), 7),
            ("MODT".to_owned(), 24)
        ]
    );
    // Nothing is lost: typed fields plus the unknown list hold every
    // subrecord of the source.
    assert_eq!(
        decoded.to_raw().subrecords.len(),
        odd_static.subrecords.len()
    );

    // A group of an unknown type is walked like any other.
    let record = walker.next_record().expect("walk").expect("reference");
    let header = walker.context().chain()[0];
    assert_eq!(
        (header.kind(), header.group_type, header.label),
        (GroupKind::Unknown, 77, *b"odd!")
    );
    assert_eq!(walker.context().top(), None);
    let Record::Placed(decoded) = record.decode().expect("decode") else {
        panic!("REFR decodes to a placed reference");
    };
    assert_eq!(decoded.base, Some(FormId(0x0100_7001)));
    assert_eq!(
        (decoded.placement, decoded.scale, decoded.teleport),
        (None, 1.0, None)
    );
    assert_eq!(decoded.enable_parent, None);
    assert_eq!(decoded.unknown, odd_ref.subrecords[1..]);
    assert!(walker.next_record().expect("walk").is_none());
}

#[test]
fn a_record_outside_any_group_is_reported_with_an_empty_context() {
    let stray = RawRecord::new(b"STAT", 0x0100_8000u32).with(b"EDID", &b"TestStray\0"[..]);
    let mut builder = PluginBuilder::new();
    builder.node(stray.clone());
    let bytes = builder.build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let items: Vec<TopItem<'_>> = plugin.top_level().map(|i| i.expect("item")).collect();
    assert!(matches!(items.as_slice(), [TopItem::Record(r)] if r.form_id().0 == 0x0100_8000));
    let mut walker = plugin.walk();
    let record = walker.next_record().expect("walk").expect("record");
    assert_eq!(walker.context().depth(), 0);
    assert_eq!(walker.context().top(), None);
    assert_eq!(record.to_raw().expect("raw"), stray);
}

#[test]
fn building_is_deterministic() {
    assert_eq!(town(), town());
}
