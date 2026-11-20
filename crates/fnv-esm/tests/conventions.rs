//! Conventions settled by the calibration run against a real master file,
//! pinned here on invented data: what a wrong checksum means, which way the
//! land grids and texture quadrants face, how navmesh edges and counts are
//! stored, and the byte layouts of the subrecords that run confirmed.

use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec, encode_record};
use fnv_esm::*;

fn first_record(bytes: &[u8]) -> RecordRef<'_> {
    Plugin::parse(bytes)
        .expect("plugin")
        .walk()
        .next_record()
        .expect("walk")
        .expect("one record")
}

/// A plugin holding one compressed script, and the offset of its record.
fn compressed_script() -> (Vec<u8>, usize, Script) {
    let mut script = Script::new(0x0100_0001, "TestChecksum", "begin GameMode\r\nend\r\n");
    script.common = script.common.with_flags(RecordFlags::COMPRESSED);
    let at = PluginBuilder::new().build().len();
    let mut bytes = PluginBuilder::new().build();
    bytes.extend_from_slice(&encode_record(&script.to_raw()));
    (bytes, at, script)
}

#[test]
fn a_wrong_checksum_is_reported_not_refused() {
    let (bytes, _, script) = compressed_script();
    let record = first_record(&bytes);
    let data = record.data().expect("inflate");
    assert!(data.was_compressed());
    assert!(data.checksum_ok());
    assert_eq!(
        data.decode().expect("decode"),
        Record::Script(script.clone())
    );
    assert_eq!(data.to_raw().expect("raw"), script.to_raw());

    // Flip a bit in each byte of the Adler-32 trailer in turn: the data is
    // whole, so the record still decodes, and the mismatch is visible.
    for back in 1..=4 {
        let mut wrong = bytes.clone();
        let at = wrong.len() - back;
        wrong[at] ^= 0x10;
        let record = first_record(&wrong);
        let data = record.data().expect("data is intact");
        assert!(!data.checksum_ok());
        assert_eq!(
            data.decode().expect("decode"),
            Record::Script(script.clone())
        );
        assert_eq!(
            record.decode().expect("decode"),
            Record::Script(script.clone())
        );
    }

    // An uncompressed record has no checksum to fail.
    let plain = Script::new(0x0100_0002, "TestPlain", "; nothing");
    let mut builder = PluginBuilder::new();
    builder.add(plain);
    let bytes = builder.build();
    let data = first_record(&bytes).data().expect("data");
    assert!(!data.was_compressed());
    assert!(data.checksum_ok());
}

#[test]
fn damage_to_the_compressed_data_itself_is_a_per_record_error() {
    // Two compressed records; the first one's deflate data is wrecked. The
    // error belongs to that record alone: the walk goes on to the second.
    let mut first = Script::new(0x0100_0001, "TestFirst", &"; filler line\r\n".repeat(20));
    first.common = first.common.with_flags(RecordFlags::COMPRESSED);
    let mut second = Script::new(0x0100_0002, "TestSecond", "; intact");
    second.common = second.common.with_flags(RecordFlags::COMPRESSED);
    let mut builder = PluginBuilder::new();
    builder.add(first).add(second.clone());
    let mut bytes = builder.build();

    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let target = walker.next_record().expect("walk").expect("first");
    // The deflate data starts after the record header, the 4-byte size and
    // the 2-byte zlib header.
    let start = target.offset + HEADER_LEN + 6;
    let end = target.offset + HEADER_LEN + target.header.data_size as usize - 4;
    bytes[start..end].fill(0xFF);

    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let wrecked = walker.next_record().expect("walk").expect("first");
    let error = wrecked.decode().expect_err("wrecked deflate data");
    assert!(matches!(error, EsmError::Decompress { .. }));
    assert_eq!(error.offset(), Some(wrecked.offset));
    assert_eq!(error.sig(), Some(sigs::SCPT));
    let intact = walker
        .next_record()
        .expect("the walk goes on")
        .expect("second");
    assert_eq!(intact.decode().expect("decode"), Record::Script(second));
    assert!(walker.next_record().expect("walk").is_none());
}

#[test]
fn quadrants_and_positions_map_onto_the_land_grid() {
    // Quadrants: 0 south-west, 1 south-east, 2 north-west, 3 north-east.
    assert_eq!(quadrant_origin(0), Some((0, 0)));
    assert_eq!(quadrant_origin(1), Some((0, 16)));
    assert_eq!(quadrant_origin(2), Some((16, 0)));
    assert_eq!(quadrant_origin(3), Some((16, 16)));
    assert_eq!(quadrant_origin(4), None);

    // Positions: row * 17 + column, row 0 south, column 0 west.
    let point = |position| AlphaPoint {
        position,
        opacity: 1.0,
    };
    assert_eq!(point(0).grid_position(), Some((0, 0)));
    assert_eq!(point(16).grid_position(), Some((0, 16)));
    assert_eq!(point(17).grid_position(), Some((1, 0)));
    assert_eq!(point(288).grid_position(), Some((16, 16)));
    assert_eq!(point(289).grid_position(), None);

    let layer = |quadrant, positions: &[u16]| AlphaLayer {
        texture: FormId(0x0100_0010),
        quadrant,
        layer: 0,
        opacity: positions.iter().map(|&p| point(p)).collect(),
    };
    let on_grid = |quadrant, positions: &[u16]| -> Vec<(usize, usize)> {
        layer(quadrant, positions)
            .vertices()
            .map(|(row, column, _)| (row, column))
            .collect()
    };
    assert_eq!(
        on_grid(0, &[0, 16, 272, 288]),
        [(0, 0), (0, 16), (16, 0), (16, 16)]
    );
    assert_eq!(on_grid(1, &[0, 16, 288]), [(0, 16), (0, 32), (16, 32)]);
    assert_eq!(on_grid(2, &[0, 17, 288]), [(16, 0), (17, 0), (32, 16)]);
    assert_eq!(on_grid(3, &[0, 288, 300]), [(16, 16), (32, 32)]);
    assert!(on_grid(7, &[0, 1]).is_empty());
}

/// An invented opacity field over world vertices: a function of the vertex
/// position in a worldspace-wide grid, so that it has one value at a vertex
/// whichever cell or quadrant stores it.
fn field(world_row: i32, world_column: i32) -> f32 {
    ((world_row * 7 + world_column * 13).rem_euclid(32)) as f32 / 32.0
}

/// The four alpha layers of the cell at `(x, y)`, sampled from `field`.
fn field_layers(x: i32, y: i32) -> Vec<AlphaLayer> {
    (0..4u8)
        .map(|quadrant| {
            let (row0, column0) = quadrant_origin(quadrant).expect("quadrant");
            AlphaLayer {
                texture: FormId(0x0100_0020),
                quadrant,
                layer: 0,
                opacity: (0..289u16)
                    .map(|position| {
                        let (row, column) =
                            (usize::from(position) / 17, usize::from(position) % 17);
                        AlphaPoint {
                            position,
                            opacity: field(
                                y * 32 + (row0 + row) as i32,
                                x * 32 + (column0 + column) as i32,
                            ),
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

#[test]
fn a_texture_is_continuous_across_quadrant_seams_and_cell_edges() {
    // Write two neighbouring cells through a plugin, read them back, and
    // put every layer entry on its cell's 33 x 33 grid. Wherever two
    // quadrants or two cells store the same vertex, they must agree.
    let cell = |form: u32, x: i32, y: i32| {
        let mut land = Land::new(form + 1);
        land.alpha_layers = field_layers(x, y);
        CellSpec::new(Cell::exterior(form, x, y)).with_land(land)
    };
    let mut builder = PluginBuilder::new();
    builder.world(
        WorldSpec::new(World::new(0x0100_0100, "TestSeams"))
            .with_cell(cell(0x0100_0110, 0, 0))
            .with_cell(cell(0x0100_0120, 1, 0))
            .with_cell(cell(0x0100_0130, 0, 1)),
    );
    let bytes = builder.build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut grids = std::collections::BTreeMap::new();
    let mut cells = std::collections::BTreeMap::new();
    while let Some(record) = walker.next_record().expect("walk") {
        match record.decode().expect("decode") {
            Record::Cell(cell) => {
                let grid = cell.grid.expect("exterior");
                cells.insert(cell.common.form_id(), (grid.x, grid.y));
            }
            Record::Land(land) => {
                let key = cells[&walker.context().cell().expect("in a cell")];
                let mut grid = vec![None; LAND_VERTICES];
                for layer in &land.alpha_layers {
                    for (row, column, opacity) in layer.vertices() {
                        let slot = &mut grid[row * LAND_GRID + column];
                        // A vertex on a quadrant seam is stored twice.
                        assert!(slot.is_none_or(|seen| seen == opacity));
                        *slot = Some(opacity);
                    }
                }
                assert!(
                    grid.iter().all(Option::is_some),
                    "the quadrants cover the cell"
                );
                grids.insert(key, grid);
            }
            _ => {}
        }
    }
    let at = |cell: (i32, i32), row: usize, column: usize| grids[&cell][row * LAND_GRID + column];
    for i in 0..LAND_GRID {
        // East edge of (0, 0) is the west edge of (1, 0).
        assert_eq!(at((0, 0), i, 32), at((1, 0), i, 0));
        // North edge of (0, 0) is the south edge of (0, 1).
        assert_eq!(at((0, 0), 32, i), at((0, 1), 0, i));
        // And each is the field's value at that world vertex.
        assert_eq!(at((1, 0), i, 0), Some(field(i as i32, 32)));
        assert_eq!(at((0, 1), 0, i), Some(field(32, i as i32)));
    }
}

#[test]
fn navmesh_edges_are_read_with_their_flag_bits() {
    let triangle = NavTriangle {
        vertices: [0, 1, 2],
        edges: [-1, 5, 3],
        // Bit 2: edge 2 is an external link.
        flags: 0b100 | 0x800,
    };
    assert_eq!(triangle.edge(0), NavEdge::Border);
    assert_eq!(triangle.edge(1), NavEdge::Triangle(5));
    assert_eq!(triangle.edge(2), NavEdge::External(3));
    assert_eq!(triangle.edge(3), NavEdge::Border);
    // A border stays a border whatever its flag bit says.
    let flagged_border = NavTriangle {
        flags: 0b001,
        ..triangle
    };
    assert_eq!(flagged_border.edge(0), NavEdge::Border);
}

#[test]
fn navmesh_data_counts_are_in_the_confirmed_order() {
    // Distinct lengths, so that each count can be told apart.
    let navmesh = Navmesh {
        cover_triangles: vec![0, 1, 2],
        door_links: vec![NavDoorLink::default(); 4],
        external_links: vec![NavExternalLink::default(); 5],
        ..Navmesh::new(
            0x0100_0200,
            0x0100_0201,
            vec![[0.0; 3]; 7],
            vec![NavTriangle::default(); 6],
        )
    };
    let raw = navmesh.to_raw();
    let data = raw
        .subrecords
        .iter()
        .find(|s| s.sig == Sig::new(b"DATA"))
        .expect("DATA");
    let words: Vec<u32> = data
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    // Cell, vertices, triangles, external links, cover triangles, doors.
    assert_eq!(words, [0x0100_0201, 7, 6, 5, 3, 4]);
    let size = |sig: &[u8; 4]| {
        raw.subrecords
            .iter()
            .find(|s| s.sig == Sig::new(sig))
            .map(|s| s.data.len())
    };
    assert_eq!(size(b"NVVX"), Some(7 * 12));
    assert_eq!(size(b"NVTR"), Some(6 * 16));
    assert_eq!(size(b"NVEX"), Some(5 * 10));
    assert_eq!(size(b"NVCA"), Some(3 * 2));
    assert_eq!(size(b"NVDP"), Some(4 * 8));
    assert_eq!(raw.decode(), Record::Navmesh(navmesh));
}

#[test]
fn exterior_block_labels_store_y_before_x() {
    // Cell (40, -9): block (1, -1), sub-block (5, -2).
    let mut builder = PluginBuilder::new();
    builder.world(
        WorldSpec::new(World::new(0x0100_0300, "TestLabels"))
            .with_cell(CellSpec::new(Cell::exterior(0x0100_0301, 40, -9))),
    );
    let bytes = builder.build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut labels = Vec::new();
    while let Some(event) = walker.next_event().expect("walk") {
        if let Event::GroupStart(header) = event
            && matches!(header.group_type, 4 | 5)
        {
            labels.push((header.group_type, header.label, header.kind()));
        }
    }
    assert_eq!(
        labels,
        [
            (
                4,
                [0xFF, 0xFF, 0x01, 0x00],
                GroupKind::ExteriorBlock { x: 1, y: -1 }
            ),
            (
                5,
                [0xFE, 0xFF, 0x05, 0x00],
                GroupKind::ExteriorSubBlock { x: 5, y: -2 }
            ),
        ]
    );
}

#[test]
fn interior_cells_are_told_by_flag_and_by_group() {
    let mut builder = PluginBuilder::new();
    builder.world(
        WorldSpec::new(World::new(0x0100_0400, "TestOutside"))
            .with_persistent_cell(CellSpec::new(Cell::exterior(0x0100_0401, 0, 0)))
            .with_cell(CellSpec::new(Cell::exterior(0x0100_0402, 2, 3))),
    );
    builder.interior(
        CellSpec::new(Cell::interior(0x0100_0403, "TestInside")).with_temporary(PlacedRef::new(
            PlacedKind::Object,
            0x0100_0404,
            0x0100_0010,
            [0.0; 3],
            [0.0; 3],
        )),
    );
    let bytes = builder.build();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut seen = Vec::new();
    while let Some(record) = walker.next_record().expect("walk") {
        let in_interior_group = walker.context().is_interior();
        match record.decode().expect("decode") {
            Record::Cell(cell) => {
                // The flag and the group agree.
                assert_eq!(cell.is_interior(), in_interior_group);
                seen.push((
                    cell.common.form_id().0,
                    cell.is_interior(),
                    cell.grid.is_some(),
                ));
            }
            Record::Placed(_) => assert!(in_interior_group),
            _ => assert!(!in_interior_group),
        }
    }
    // The worldspace's own persistent cell has a grid position and is not
    // an interior, so "no grid" and "interior" are the same set here too.
    assert_eq!(
        seen,
        [
            (0x0100_0401, false, true),
            (0x0100_0402, false, true),
            (0x0100_0403, true, false)
        ]
    );
    // A cell with no DATA at all is not called an interior.
    assert!(!Cell::default().is_interior());
}

#[test]
fn reference_extras_have_the_confirmed_layouts() {
    let mut xprm = Vec::new();
    for value in [64.0f32, 128.0, 96.0, 0.25, 0.5, 1.0, 0.75] {
        xprm.extend_from_slice(&value.to_le_bytes());
    }
    xprm.extend_from_slice(&3u32.to_le_bytes());
    let raw = RawRecord::new(b"REFR", 0x0100_0500u32)
        .with(b"NAME", 0x0100_0010u32.to_le_bytes())
        .with(b"XPRM", xprm.clone())
        .with(b"XRDS", (-32.0f32).to_le_bytes())
        .with(b"XEMI", 0x0100_0011u32.to_le_bytes());
    let Record::Placed(placed) = raw.decode() else {
        panic!("REFR decodes to a placed reference");
    };
    assert_eq!(
        placed.primitive,
        Some(Primitive {
            bounds: [64.0, 128.0, 96.0],
            color: [0.25, 0.5, 1.0],
            unknown: 0.75,
            shape: Primitive::SHAPE_PORTAL_BOX,
        })
    );
    assert_eq!(placed.radius, Some(-32.0));
    assert_eq!(placed.emittance, Some(FormId(0x0100_0011)));
    assert!(placed.unknown.is_empty());
    let written = placed.to_raw();
    let find = |sig: &[u8; 4]| {
        written
            .subrecords
            .iter()
            .find(|s| s.sig == Sig::new(sig))
            .map(|s| s.data.clone())
    };
    assert_eq!(find(b"XPRM"), Some(xprm));
    assert_eq!(find(b"XRDS"), Some((-32.0f32).to_le_bytes().to_vec()));

    // Sizes the decoder has no layout for stay raw.
    let odd = RawRecord::new(b"REFR", 1u32)
        .with(b"XPRM", vec![0; 28])
        .with(b"XRDS", vec![0; 8])
        .with(b"XEMI", vec![0; 3]);
    let Record::Placed(placed) = odd.decode() else {
        panic!("REFR decodes to a placed reference");
    };
    assert_eq!(
        (placed.primitive, placed.radius, placed.emittance),
        (None, None, None)
    );
    assert_eq!(placed.unknown, odd.subrecords);
}

#[test]
fn reference_flags_are_methods() {
    let plain = PlacedRef::new(PlacedKind::Object, 1u32, 2u32, [0.0; 3], [0.0; 3]);
    assert!(!plain.is_persistent() && !plain.is_initially_disabled() && !plain.is_deleted());
    let flagged = plain.clone().persistent().initially_disabled();
    assert!(flagged.is_persistent() && flagged.is_initially_disabled());
    assert!(!flagged.is_deleted());

    let mut raw = plain.to_raw();
    raw.meta.flags.0 |= RecordFlags::DELETED;
    let Record::Placed(deleted) = raw.decode() else {
        panic!("REFR decodes to a placed reference");
    };
    assert!(deleted.is_deleted());
    assert!(!deleted.is_initially_disabled());
}

#[test]
fn the_short_ammunition_form_defaults_its_tail() {
    let mut dat2 = 8u32.to_le_bytes().to_vec();
    dat2.extend_from_slice(&0x0100_0600u32.to_le_bytes());
    dat2.extend_from_slice(&0.125f32.to_le_bytes());
    let short = RawRecord::new(b"AMMO", 0x0100_0601u32).with(b"DAT2", dat2.clone());
    let Record::Ammo(ammo) = short.decode() else {
        panic!("AMMO decodes to ammunition");
    };
    assert_eq!(
        ammo.data2,
        Some(AmmoData2 {
            projectiles_per_shot: 8,
            projectile: FormId(0x0100_0600),
            weight: 0.125,
            consumed_ammo: FormId::NULL,
            consumed_percentage: 0.0,
        })
    );
    assert!(ammo.unknown.is_empty());

    // The full form: the casing and the chance of leaving it, in percent.
    dat2.extend_from_slice(&0x0100_0602u32.to_le_bytes());
    dat2.extend_from_slice(&35.0f32.to_le_bytes());
    let full = RawRecord::new(b"AMMO", 0x0100_0601u32).with(b"DAT2", dat2);
    let Record::Ammo(ammo) = full.decode() else {
        panic!("AMMO decodes to ammunition");
    };
    let data2 = ammo.data2.expect("DAT2");
    assert_eq!(data2.consumed_ammo, FormId(0x0100_0602));
    assert_eq!(data2.consumed_percentage, 35.0);
    assert_eq!(ammo.to_raw(), full);

    // Any other size stays raw.
    let odd = RawRecord::new(b"AMMO", 1u32).with(b"DAT2", vec![0; 16]);
    let Record::Ammo(ammo) = odd.decode() else {
        panic!("AMMO decodes to ammunition");
    };
    assert_eq!(ammo.data2, None);
    assert_eq!(ammo.unknown.len(), 1);
}

#[test]
fn armour_and_lock_layouts_are_as_confirmed() {
    // ARMO DNAM, 12 bytes: rating, two padding bytes, threshold, flags,
    // two more padding bytes. Padding may hold anything.
    let mut dnam = 150i16.to_le_bytes().to_vec();
    dnam.extend_from_slice(&[0xAA, 0xBB]);
    dnam.extend_from_slice(&12.0f32.to_le_bytes());
    dnam.extend_from_slice(&1u16.to_le_bytes());
    dnam.extend_from_slice(&[0, 0]);
    let Record::Armor(armor) = RawRecord::new(b"ARMO", 1u32).with(b"DNAM", dnam).decode() else {
        panic!("ARMO decodes to armour");
    };
    assert_eq!(
        armor.stats,
        Some(ArmorStats {
            armor_rating: 150,
            damage_threshold: Some(12.0),
            flags: 1,
        })
    );

    // XLOC, 20 bytes: level, three padding bytes, key, flags, three padding
    // bytes, eight zero bytes. The padding holds leftovers in real files.
    let mut xloc = vec![75, 0xDE, 0xAD, 0xBE];
    xloc.extend_from_slice(&0x0100_0700u32.to_le_bytes());
    xloc.extend_from_slice(&[0x04, 0xEF, 0xCD, 0xAB]);
    xloc.extend_from_slice(&[0; 8]);
    let Record::Placed(placed) = RawRecord::new(b"REFR", 1u32).with(b"XLOC", xloc).decode() else {
        panic!("REFR decodes to a placed reference");
    };
    assert_eq!(
        placed.lock,
        Some(Lock {
            level: 75,
            key: FormId(0x0100_0700),
            flags: 4,
        })
    );
}
