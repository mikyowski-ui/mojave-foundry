//! Malformed input: truncation, absurd sizes, bad compression, deep
//! nesting and random corruption. Everything is an error value with an
//! offset and a signature; nothing panics, loops or allocates by the size a
//! file merely claims.

use fnv_esm::fixture::{
    CellSpec, GroupNode, Node, PluginBuilder, WorldSpec, encode_record, flat_heights, write_node,
};
use fnv_esm::*;

/// Parses and walks everything, decoding every record. Returns the number
/// of records decoded.
fn exercise(bytes: &[u8]) -> Result<usize, EsmError> {
    let plugin = Plugin::parse(bytes)?;
    for item in plugin.top_level() {
        item?;
    }
    let mut walker = plugin.walk();
    let mut records = 0;
    let mut events = 0usize;
    while let Some(event) = walker.next_event()? {
        events += 1;
        // Every event consumes at least one 24-byte header.
        assert!(
            events <= bytes.len() / 12 + 2,
            "the walk does not terminate"
        );
        if let Event::Record(record) = event {
            record.data_with_limit(1 << 20)?.collect()?;
            record.decode()?;
            records += 1;
        }
    }
    Ok(records)
}

fn small_plugin() -> Vec<u8> {
    let mut builder = PluginBuilder::new();
    builder.author("malformed test").master("Base.esm");
    builder.add(Static::new(0x0100_1000, "TestPost", "clutter\\post.nif"));
    let mut packed = Script::new(0x0100_1001, "TestPackedScript", &"; padding\r\n".repeat(30));
    packed.common = packed.common.with_flags(RecordFlags::COMPRESSED);
    builder.add(packed);
    builder.add_raw(RawRecord::new(b"NAVM", 0x0100_1002u32).with(b"NVGD", vec![7u8; 66_000]));
    builder.world(
        WorldSpec::new(World::new(0x0100_2000, "TestWorld")).with_cell(
            CellSpec::new(Cell::exterior(0x0100_2001, 0, 0))
                .with_land(Land::new(0x0100_2002).with_heights(&flat_heights(0.0)))
                .with_temporary(PlacedRef::new(
                    PlacedKind::Object,
                    0x0100_2003,
                    0x0100_1000,
                    [1.0, 2.0, 3.0],
                    [0.0; 3],
                )),
        ),
    );
    builder.topic(
        Topic::new(0x0100_3000, "TestTopic", "Hello", 0x0100_3100u32),
        vec![Info::new(
            0x0100_3001,
            0x0100_3100u32,
            vec![Response::new(1, "Hi.")],
        )],
    );
    builder.build()
}

/// A plugin holding just `body` after an empty header.
fn with_body(body: &[u8]) -> Vec<u8> {
    let mut bytes = PluginBuilder::new().build();
    bytes.extend_from_slice(body);
    bytes
}

fn header_len() -> usize {
    PluginBuilder::new().build().len()
}

fn le(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

/// A raw 24-byte record header.
fn record_header(sig: &[u8; 4], data_size: u32, flags: u32) -> Vec<u8> {
    let mut out = sig.to_vec();
    out.extend_from_slice(&le(data_size));
    out.extend_from_slice(&le(flags));
    out.extend_from_slice(&[0; 12]);
    out
}

/// A raw 24-byte group header.
fn group_header(size: u32) -> Vec<u8> {
    let mut out = b"GRUP".to_vec();
    out.extend_from_slice(&le(size));
    out.extend_from_slice(b"STAT");
    out.extend_from_slice(&[0; 12]);
    out
}

/// A record with the given raw data, to be read as subrecords.
fn record_with_data(data: &[u8]) -> Vec<u8> {
    let mut out = record_header(b"STAT", data.len() as u32, 0);
    out.extend_from_slice(data);
    out
}

#[test]
fn the_intact_plugin_is_fine() {
    assert_eq!(exercise(&small_plugin()), Ok(9));
}

#[test]
fn every_truncation_is_an_error_or_a_shorter_valid_file() {
    let bytes = small_plugin();
    let full = exercise(&bytes).expect("intact");
    let mut errors = 0;
    for len in 0..bytes.len() {
        match exercise(&bytes[..len]) {
            // A cut on a top-level boundary leaves a valid, shorter plugin.
            Ok(records) => assert!(records < full, "prefix {len} lost nothing"),
            Err(error) => {
                errors += 1;
                let offset = error.offset().expect("parse errors carry an offset");
                assert!(offset <= len, "offset {offset} past the end {len}");
                assert!(error.sig().is_some());
                assert!(!error.to_string().is_empty());
            }
        }
    }
    // Only the cuts between top-level groups parse: header, STAT, SCPT,
    // NAVM, WRLD (DIAL is the last, so its cut is the whole file).
    assert_eq!(bytes.len() - errors, 5);
}

#[test]
fn every_truncation_of_a_record_body_is_an_error_or_a_shorter_record() {
    // Cutting a whole file trips the top-level size checks first. Here each
    // record's data is cut on its own, with a header that matches the cut,
    // so the subrecord framing is what has to notice.
    let bytes = small_plugin();
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let mut walker = plugin.walk();
    let mut checked = 0;
    while let Some(record) = walker.next_record().expect("walk") {
        let data = record.data().expect("data");
        let data = data.bytes();
        if data.len() > 10_000 {
            continue;
        }
        let subrecords = record.to_raw().expect("raw").subrecords.len();
        let mut clean_cuts = 0;
        for len in 0..data.len() {
            let mut body = record_header(record.sig().as_bytes(), len as u32, 0);
            body.extend_from_slice(&data[..len]);
            let cut = with_body(&body);
            let cut_plugin = Plugin::parse(&cut).expect("plugin");
            let cut_record = cut_plugin
                .walk()
                .next_record()
                .expect("walk")
                .expect("record");
            match cut_record.decode() {
                Ok(decoded) => {
                    clean_cuts += 1;
                    assert!(decoded.to_raw().subrecords.len() < subrecords);
                }
                Err(error) => {
                    assert_eq!(error.sig(), Some(record.sig()));
                    let offset = error.offset().expect("offset");
                    assert!(offset >= header_len() + HEADER_LEN && offset <= cut.len());
                }
            }
        }
        // Only a cut between two subrecords leaves a readable record.
        assert_eq!(clean_cuts, subrecords, "{}", record.sig());
        checked += 1;
    }
    assert_eq!(checked, 8);
}

#[test]
fn a_file_too_short_for_a_header_is_truncated() {
    assert_eq!(
        Plugin::parse(&[]).map(|_| ()),
        Err(EsmError::Truncated {
            offset: 0,
            sig: Sig::new(b"????"),
            what: "record or group header",
            needed: 24,
            remaining: 0,
        })
    );
    let error = Plugin::parse(b"TES4\x10\x00").expect_err("six bytes");
    assert_eq!(error.sig(), Some(sigs::TES4));
    assert_eq!(error.offset(), Some(0));
}

#[test]
fn a_file_that_does_not_start_with_tes4_is_not_a_plugin() {
    let stat = encode_record(&RawRecord::new(b"STAT", 1u32));
    assert_eq!(
        Plugin::parse(&stat).map(|_| ()),
        Err(EsmError::NotAPlugin {
            offset: 0,
            sig: sigs::STAT
        })
    );
    let mut group = Vec::new();
    write_node(&mut group, &Node::Group(GroupNode::top(sigs::STAT)), &mut 0);
    assert_eq!(
        Plugin::parse(&group).map(|_| ()),
        Err(EsmError::NotAPlugin {
            offset: 0,
            sig: sigs::GRUP
        })
    );
    assert!(matches!(
        Plugin::parse(&[0xFF; 64]),
        Err(EsmError::Truncated {
            what: "record data",
            ..
        })
    ));
}

#[test]
fn a_header_without_hedr_is_rejected() {
    let no_hedr = encode_record(&RawRecord::new(b"TES4", 0u32).with(b"CNAM", &b"x\0"[..]));
    assert_eq!(
        Plugin::parse(&no_hedr).map(|_| ()),
        Err(EsmError::MissingHedr {
            offset: 0,
            sig: sigs::TES4
        })
    );
    let short_hedr = encode_record(&RawRecord::new(b"TES4", 0u32).with(b"HEDR", vec![0; 11]));
    assert!(matches!(
        Plugin::parse(&short_hedr),
        Err(EsmError::MissingHedr { .. })
    ));
    // A TES4 whose own subrecords are cut short is a framing error.
    let mut bad = record_header(b"TES4", 8, 0);
    bad.extend_from_slice(b"HEDR\x0C\x00\x00\x00");
    assert!(matches!(
        Plugin::parse(&bad),
        Err(EsmError::Truncated { what: "subrecord data", sig, .. }) if sig == sigs::TES4
    ));
}

#[test]
fn an_absurd_record_size_is_an_error_at_that_record() {
    let at = header_len();
    for claimed in [1u32, 25, 0x7FFF_FFFF, 0xFFFF_FFFF] {
        let bytes = with_body(&record_header(b"STAT", claimed, 0));
        let plugin = Plugin::parse(&bytes).expect("header");
        let mut walker = plugin.walk();
        assert_eq!(
            walker.next_event().map(|_| ()),
            Err(EsmError::Truncated {
                offset: at,
                sig: sigs::STAT,
                what: "record data",
                needed: u64::from(claimed),
                remaining: 0,
            })
        );
        // After an error the walk is over.
        assert!(matches!(walker.next_event(), Ok(None)));
        assert!(matches!(plugin.top_level().next(), Some(Err(_))));
    }
}

#[test]
fn an_absurd_group_size_is_an_error_at_that_group() {
    let at = header_len();
    for size in [0u32, 1, 23] {
        let bytes = with_body(&group_header(size));
        assert_eq!(
            exercise(&bytes),
            Err(EsmError::GroupSizeTooSmall {
                offset: at,
                sig: sigs::GRUP,
                size
            })
        );
    }
    for size in [25u32, 0x0100_0000, 0xFFFF_FFFF] {
        let bytes = with_body(&group_header(size));
        assert_eq!(
            exercise(&bytes),
            Err(EsmError::Truncated {
                offset: at,
                sig: sigs::GRUP,
                what: "group contents",
                needed: u64::from(size),
                remaining: 24,
            })
        );
    }
    // An empty group is valid.
    assert_eq!(exercise(&with_body(&group_header(24))), Ok(0));
}

#[test]
fn a_child_may_not_run_past_its_group() {
    // A group of 24 + 24 bytes holding a record that claims 24 bytes of
    // data. Those bytes exist in the file, but they are the next group.
    let mut body = group_header(48);
    body.extend_from_slice(&record_header(b"STAT", 24, 0));
    body.extend_from_slice(&group_header(24));
    let at = header_len() + 24;
    assert_eq!(
        exercise(&with_body(&body)),
        Err(EsmError::Truncated {
            offset: at,
            sig: sigs::STAT,
            what: "record data",
            needed: 24,
            remaining: 0,
        })
    );
    // And a nested group may not either.
    let mut body = group_header(48);
    body.extend_from_slice(&group_header(48));
    body.extend_from_slice(&group_header(24));
    assert!(matches!(
        exercise(&with_body(&body)),
        Err(EsmError::Truncated { what: "group contents", offset, .. }) if offset == at
    ));
    // A group with room for part of a header only.
    let mut body = group_header(30);
    body.extend_from_slice(b"STAT\x00\x00");
    assert!(matches!(
        exercise(&with_body(&body)),
        Err(EsmError::Truncated { what: "record or group header", remaining: 6, sig, .. })
            if sig == sigs::STAT
    ));
}

#[test]
fn groups_nested_too_deep_are_refused() {
    let nest = |depth: usize| {
        let mut node = GroupNode::top(sigs::STAT).with(RawRecord::new(b"STAT", 1u32));
        for _ in 1..depth {
            node = GroupNode::top(sigs::STAT).with(node);
        }
        let mut builder = PluginBuilder::new();
        builder.node(node);
        builder.build()
    };
    assert_eq!(exercise(&nest(MAX_GROUP_DEPTH)), Ok(1));
    let too_deep = nest(MAX_GROUP_DEPTH + 1);
    let error = exercise(&too_deep).expect_err("17 groups deep");
    assert_eq!(
        error,
        EsmError::GroupNestingTooDeep {
            offset: header_len() + 24 * MAX_GROUP_DEPTH,
            sig: sigs::GRUP,
            limit: MAX_GROUP_DEPTH,
        }
    );
    // Far deeper costs nothing more: the walk stops at the limit.
    assert!(matches!(
        exercise(&nest(5000)),
        Err(EsmError::GroupNestingTooDeep { .. })
    ));
}

#[test]
fn subrecord_sizes_are_checked_against_the_record() {
    let data_at = header_len() + 24;
    let first =
        |data: &[u8]| -> Result<usize, EsmError> { exercise(&with_body(&record_with_data(data))) };

    // A size field larger than what is left.
    assert_eq!(
        first(b"EDID\xFF\xFFabc"),
        Err(EsmError::Truncated {
            offset: data_at + 6,
            sig: sigs::STAT,
            what: "subrecord data",
            needed: 0xFFFF,
            remaining: 3,
        })
    );
    // Fewer than six bytes left for a header.
    assert_eq!(
        first(b"EDID\x01\x00xMOD"),
        Err(EsmError::Truncated {
            offset: data_at + 7,
            sig: sigs::STAT,
            what: "subrecord header",
            needed: 6,
            remaining: 3,
        })
    );
    // Zero-length subrecords are fine and make progress.
    assert_eq!(first(&b"EDID\x00\x00".repeat(1000)), Ok(1));
}

#[test]
fn the_extended_size_escape_is_checked() {
    let data_at = header_len() + 24;
    let first =
        |data: &[u8]| -> Result<usize, EsmError> { exercise(&with_body(&record_with_data(data))) };

    // An extended size far beyond the record: an error, not an allocation.
    let mut data = b"XXXX\x04\x00".to_vec();
    data.extend_from_slice(&le(0xFFFF_FFFF));
    data.extend_from_slice(b"MODT\x00\x00tiny");
    assert_eq!(
        first(&data),
        Err(EsmError::Truncated {
            offset: data_at + 16,
            sig: sigs::STAT,
            what: "subrecord data",
            needed: 0xFFFF_FFFF,
            remaining: 4,
        })
    );
    // A marker whose own size is not 4.
    assert_eq!(
        first(b"XXXX\x02\x00\x10\x00MODT\x00\x00"),
        Err(EsmError::BadExtendedSize {
            offset: data_at,
            sig: sigs::STAT,
            reason: "the marker's own size is not 4",
        })
    );
    // A marker with nothing after it.
    let mut data = b"EDID\x01\x00\x00XXXX\x04\x00".to_vec();
    data.extend_from_slice(&le(16));
    assert_eq!(
        first(&data),
        Err(EsmError::BadExtendedSize {
            offset: data_at + 7,
            sig: sigs::STAT,
            reason: "no subrecord follows the marker",
        })
    );
    // A marker cut off inside its size.
    assert!(matches!(
        first(b"XXXX\x04\x00\x10\x00"),
        Err(EsmError::Truncated {
            what: "extended subrecord size",
            needed: 4,
            remaining: 2,
            ..
        })
    ));
    // A marker followed by a cut-off header.
    let mut data = b"XXXX\x04\x00".to_vec();
    data.extend_from_slice(&le(1));
    data.extend_from_slice(b"MOD");
    assert!(matches!(
        first(&data),
        Err(EsmError::Truncated {
            what: "subrecord header",
            ..
        })
    ));
    // The size field after a marker is ignored, whatever it says.
    let mut data = b"XXXX\x04\x00".to_vec();
    data.extend_from_slice(&le(3));
    data.extend_from_slice(b"MODT\xEE\xEEabc");
    let bytes = with_body(&record_with_data(&data));
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let record = plugin.walk().next_record().expect("walk").expect("record");
    assert_eq!(
        record.to_raw().expect("raw").subrecords,
        [RawSubrecord::new(b"MODT", &b"abc"[..])]
    );
}

/// A compressed record: `declared` as the size prefix, then `stream`.
fn compressed_record(declared: u32, stream: &[u8]) -> Vec<u8> {
    let mut out = record_header(b"SCPT", 4 + stream.len() as u32, RecordFlags::COMPRESSED);
    out.extend_from_slice(&le(declared));
    out.extend_from_slice(stream);
    out
}

/// The zlib stream the fixture writes for `size` zero bytes of record data.
fn zlib_of_zeros(size: usize) -> Vec<u8> {
    let mut raw = RawRecord::new(b"SCPT", 0u32).with(b"SCDA", vec![0u8; size - 6]);
    raw.meta.flags.0 |= RecordFlags::COMPRESSED;
    encode_record(&raw)[HEADER_LEN + 4..].to_vec()
}

fn first_record_data(body: &[u8], limit: usize) -> Result<usize, EsmError> {
    let bytes = with_body(body);
    let plugin = Plugin::parse(&bytes)?;
    let record = plugin.walk().next_record()?.expect("one record");
    Ok(record.data_with_limit(limit)?.bytes().len())
}

#[test]
fn decompressed_size_is_capped_before_anything_is_inflated() {
    let at = header_len();
    let stream = zlib_of_zeros(4096);
    assert_eq!(
        first_record_data(&compressed_record(4096, &stream), 4096),
        Ok(4096)
    );
    assert_eq!(
        first_record_data(&compressed_record(4096, &stream), 4095),
        Err(EsmError::DecompressedTooLarge {
            offset: at,
            sig: sigs::SCPT,
            declared: 4096,
            limit: 4095,
        })
    );
    // A record claiming 4 GiB is refused by the default cap without the
    // stream being looked at.
    let bytes = with_body(&compressed_record(0xFFFF_FFFF, b"not zlib at all"));
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let record = plugin.walk().next_record().expect("walk").expect("record");
    assert_eq!(
        record.data().map(|_| ()),
        Err(EsmError::DecompressedTooLarge {
            offset: at,
            sig: sigs::SCPT,
            declared: 0xFFFF_FFFF,
            limit: DEFAULT_MAX_DECOMPRESSED as u64,
        })
    );
    assert!(matches!(
        record.decode(),
        Err(EsmError::DecompressedTooLarge { .. })
    ));
    assert!(matches!(
        record.to_raw(),
        Err(EsmError::DecompressedTooLarge { .. })
    ));
}

#[test]
fn a_stream_that_expands_past_its_declared_size_is_stopped() {
    let at = header_len();
    // 8 MiB of zeros behind a prefix that declares 100 bytes: inflation
    // stops one byte past the declared size.
    let bomb = zlib_of_zeros(8 << 20);
    assert!(bomb.len() < 16 << 10);
    assert_eq!(
        first_record_data(&compressed_record(100, &bomb), 1 << 30),
        Err(EsmError::DecompressedSizeMismatch {
            offset: at,
            sig: sigs::SCPT,
            declared: 100,
            actual: 101,
        })
    );
    // A stream shorter than declared.
    let stream = zlib_of_zeros(64);
    assert_eq!(
        first_record_data(&compressed_record(65, &stream), 1 << 30),
        Err(EsmError::DecompressedSizeMismatch {
            offset: at,
            sig: sigs::SCPT,
            declared: 65,
            actual: 64,
        })
    );
    // A stream cut off before its end marker.
    for cut in [5, 6, stream.len() - 2] {
        let cut = &stream[..stream.len() - cut];
        assert!(matches!(
            first_record_data(&compressed_record(64, cut), 1 << 30),
            Err(EsmError::Decompress { .. })
        ));
    }
    // The four bytes after the end marker are the checksum. Whole data
    // with a wrong, cut or missing checksum is still data: see
    // `tests/conventions.rs` for how that is reported.
    for cut in [1, 3, 4] {
        let cut = &stream[..stream.len() - cut];
        assert_eq!(
            first_record_data(&compressed_record(64, cut), 1 << 30),
            Ok(64)
        );
    }
    // A stream that does not start with a zlib header.
    for (at, value) in [(0, 0x79), (0, 0x88), (1, 0x9D), (1, 0xBB)] {
        let mut wrong = stream.clone();
        wrong[at] = value;
        assert!(matches!(
            first_record_data(&compressed_record(64, &wrong), 1 << 30),
            Err(EsmError::Decompress { .. })
        ));
    }
    assert!(matches!(
        first_record_data(&compressed_record(64, &stream[..1]), 1 << 30),
        Err(EsmError::Decompress { .. })
    ));
    // Bytes after the end marker are ignored.
    let mut padded = stream.clone();
    padded.extend_from_slice(&[0xEE; 9]);
    assert_eq!(
        first_record_data(&compressed_record(64, &padded), 1 << 30),
        Ok(64)
    );
}

#[test]
fn a_bad_zlib_stream_is_an_error() {
    let at = header_len();
    let error = first_record_data(&compressed_record(32, &[0xFF; 40]), 1 << 20)
        .expect_err("not a zlib stream");
    assert!(
        matches!(&error, EsmError::Decompress { offset, sig, .. } if *offset == at && *sig == sigs::SCPT),
        "{error:?}"
    );
    // No stream at all, and no room for the size prefix.
    assert!(first_record_data(&compressed_record(8, &[]), 1 << 20).is_err());
    assert_eq!(
        first_record_data(&record_header(b"SCPT", 0, RecordFlags::COMPRESSED), 1 << 20),
        Err(EsmError::Truncated {
            offset: at,
            sig: sigs::SCPT,
            what: "compressed record size prefix",
            needed: 4,
            remaining: 0,
        })
    );
    // Declaring zero bytes does not excuse a missing stream, but an empty
    // stream of zero bytes is a valid, empty record.
    assert!(first_record_data(&compressed_record(0, &[]), 16).is_err());
    let mut empty = RawRecord::new(b"SCPT", 0u32);
    empty.meta.flags.0 |= RecordFlags::COMPRESSED;
    assert_eq!(first_record_data(&encode_record(&empty), 16), Ok(0));
}

#[test]
fn garbage_inside_a_compressed_record_is_reported_at_the_record() {
    // Valid zlib around subrecord data that is cut short.
    let mut raw = RawRecord::new(b"SCPT", 5u32).with(b"SCTX", &b"short x"[..]);
    raw.meta.flags.0 |= RecordFlags::COMPRESSED;
    let good = encode_record(&raw);
    let bytes = with_body(&good);
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let record = plugin.walk().next_record().expect("walk").expect("record");
    assert!(record.decode().is_ok());

    // Re-pack the same data minus its last three bytes.
    let data = record.data().expect("inflate");
    let cut = &data.bytes()[..data.bytes().len() - 3];
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut encoder, cut).expect("write");
    let stream = encoder.finish().expect("finish");
    let bytes = with_body(&compressed_record(cut.len() as u32, &stream));
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let record = plugin.walk().next_record().expect("walk").expect("record");
    assert_eq!(
        record.decode().map(|_| ()),
        Err(EsmError::Truncated {
            offset: header_len(),
            sig: sigs::SCPT,
            what: "subrecord data",
            needed: 7,
            remaining: 4,
        })
    );
}

/// A small deterministic generator, so that the corruption runs are the
/// same on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, bound: usize) -> usize {
        self.next() as usize % bound
    }
}

#[test]
fn random_corruption_never_panics() {
    let clean = small_plugin();
    let mut rng = Lcg(0x5EED);
    let mut failures = 0;
    for _ in 0..3000 {
        let mut bytes = clean.clone();
        for _ in 0..=rng.below(4) {
            let at = rng.below(bytes.len());
            match rng.below(4) {
                0 => bytes[at] = rng.next() as u8,
                1 => bytes[at] = 0xFF,
                2 => bytes[at] = 0,
                _ => bytes[at] ^= 1 << rng.below(8),
            }
        }
        if exercise(&bytes).is_err() {
            failures += 1;
        }
    }
    // Most single-byte hits land in payloads and are harmless; the ones on
    // sizes and signatures are caught.
    assert!(failures > 0);
    assert!(failures < 3000);
}

#[test]
fn typed_decoders_take_any_bytes() {
    // Every subrecord signature a decoder looks at, with payloads of every
    // length around the sizes they expect, in every record type.
    const SUBRECORDS: &[&[u8; 4]] = &[
        b"EDID", b"FULL", b"MODL", b"SCRI", b"OBND", b"DATA", b"DNAM", b"XCLC", b"XCLL", b"XCLW",
        b"XCLR", b"VHGT", b"VNML", b"VCLR", b"BTXT", b"ATXT", b"VTXT", b"VTEX", b"NVER", b"NVVX",
        b"NVTR", b"NVCA", b"NVDP", b"NVEX", b"NAME", b"XTEL", b"XESP", b"XLOC", b"XSCL", b"XCNT",
        b"CNTO", b"CRDT", b"BMDT", b"ENIT", b"EFID", b"EFIT", b"CTDA", b"DAT2", b"ACBS", b"SNAM",
        b"NIFZ", b"KFFZ", b"HCLR", b"FGGS", b"INDX", b"QSDT", b"CNAM", b"QOBJ", b"NNAM", b"QSTA",
        b"SCHR", b"SCDA", b"SCTX", b"SLSD", b"SCVR", b"SCRO", b"SCRV", b"NEXT", b"TRDT", b"NAM1",
        b"QSTI", b"TCLT", b"MNAM", b"ONAM", b"NAM0", b"HNAM", b"TX00", b"TNAM", b"GNAM", b"PNAM",
        b"FNAM", b"ICON", b"ENAM", b"DESC", b"MOD3", b"BIPL", b"ICO2", b"LNAM", b"LVLD", b"LVLF",
        b"LVLG", b"LVLO", b"COED",
    ];
    let mut rng = Lcg(0xD1CE);
    let mut decoded = 0usize;
    for entry in DECODERS {
        // One subrecord at a time, every length from 0 to 48.
        for sig in SUBRECORDS {
            for len in 0..=48usize {
                let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                let raw = RawRecord {
                    sig: entry.sig,
                    meta: RecordMeta::new(1u32),
                    subrecords: vec![RawSubrecord::new(sig, data)],
                };
                let record = raw.decode();
                assert_eq!(record.sig(), entry.sig);
                // Either the decoder took it, or it is kept exactly.
                let unknown = record.unknown_subrecords();
                assert!(unknown.is_empty() || unknown == raw.subrecords.as_slice());
                let _ = record.to_raw();
                decoded += 1;
            }
        }
        // Random sequences, including the stateful orders (stages before
        // entries, VTXT without ATXT, effects without EFID and so on).
        for _ in 0..300 {
            let subrecords: Vec<RawSubrecord> = (0..rng.below(12))
                .map(|_| {
                    let sig = SUBRECORDS[rng.below(SUBRECORDS.len())];
                    let len = [0, 1, 2, 4, 8, 12, 16, 20, 24, 28, 32, 1096][rng.below(12)];
                    RawSubrecord::new(sig, (0..len).map(|_| rng.next() as u8).collect::<Vec<u8>>())
                })
                .collect();
            let raw = RawRecord {
                sig: entry.sig,
                meta: RecordMeta::new(2u32),
                subrecords,
            };
            let record = raw.decode();
            // Re-encoding and decoding again does not panic either.
            let _ = record.to_raw().decode();
            decoded += 1;
        }
    }
    assert_eq!(decoded, DECODERS.len() * (SUBRECORDS.len() * 49 + 300));
}

#[test]
fn land_grids_of_the_wrong_size_stay_raw() {
    let raw = RawRecord::new(b"LAND", 9u32)
        .with(b"VHGT", vec![0u8; 1095])
        .with(b"VNML", vec![0u8; 3266])
        .with(b"VCLR", vec![0u8; 3268])
        .with(b"VTXT", vec![0u8; 8])
        .with(b"ATXT", vec![0u8; 8])
        .with(b"VTXT", vec![0u8; 12]);
    let Record::Land(land) = raw.decode() else {
        panic!("LAND decodes to a land");
    };
    assert_eq!(
        (land.heights, land.normals, land.colors),
        (None, None, None)
    );
    assert_eq!(land.alpha_layers.len(), 1);
    assert!(land.alpha_layers[0].opacity.is_empty());
    let kept: Vec<usize> = land.unknown.iter().map(|s| s.data.len()).collect();
    assert_eq!(kept, [1095, 3266, 3268, 8, 12]);
}
