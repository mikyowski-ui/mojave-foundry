//! MSG and GAM writer round trips through `MsgCatalog::parse_strict` and
//! `GamFile::parse`.

use emit::gam::{GAM_LINE_BYTES, write_gam};
use emit::msg::{MSG_FIELD_BYTES, write_msg};
use emit::{EmitError, FormatWriters, GamSection, GamVar, LocalWriters, MsgEntry, MsgFile};
use vault_formats::{GamFile, MsgCatalog};

fn entry(id: i32, sound: &[u8], text: &[u8]) -> MsgEntry {
    MsgEntry {
        id,
        sound: sound.to_vec(),
        text: text.to_vec(),
    }
}

fn check(entries: &[MsgEntry]) {
    let bytes = LocalWriters.msg(entries).expect("writes");
    let catalog = MsgCatalog::parse_strict(&bytes).expect("parses");
    assert_eq!(catalog.entries().len(), entries.len());
    for expected in entries {
        let got = catalog.get(expected.id).expect("present");
        assert_eq!(got.sound, expected.sound, "id {}", expected.id);
        assert_eq!(got.text, expected.text, "id {}", expected.id);
    }
    // The runtime loader agrees and reports no failure.
    assert!(MsgCatalog::parse(&bytes).unwrap().failure().is_none());
}

#[test]
fn messages_round_trip_across_edge_cases() {
    check(&[
        entry(100, b"", b"A name"),
        entry(
            101,
            b"snd01",
            b"A description with {{ opening braces and ; # // text",
        ),
        entry(-5, b"", b""),
        entry(0, b"x", b"bare\rcarriage return"),
        entry(i32::MAX, b"", &[0x80, 0x9f, 0xc0, 0xff]),
        entry(i32::MIN, b"", b"  leading and trailing spaces  "),
        entry(7, &[b's'; MSG_FIELD_BYTES], &[b't'; MSG_FIELD_BYTES]),
        entry(8, b"", b"ends with cr\r"),
    ]);
    check(&[]);
    assert_eq!(write_msg(&[]).unwrap(), b"");
}

#[test]
fn messages_are_written_in_id_order() {
    let bytes = write_msg(&[
        entry(3, b"", b"c"),
        entry(-1, b"", b"a"),
        entry(2, b"", b"b"),
    ])
    .unwrap();
    assert_eq!(bytes, b"{-1}{}{a}\n{2}{}{b}\n{3}{}{c}\n");
}

#[test]
fn the_msg_writer_refuses_what_it_cannot_carry() {
    for bad in [
        entry(1, b"", b"close } brace"),
        entry(1, b"", b"line\nfeed"),
        entry(1, b"", b"nul\0byte"),
        entry(1, b"", b"eof\x1amark"),
        entry(1, b"}", b""),
        entry(1, b"", &[b'x'; MSG_FIELD_BYTES + 1]),
    ] {
        assert!(matches!(
            write_msg(&[bad]),
            Err(EmitError::Msg { id: 1, .. })
        ));
    }
    assert!(matches!(
        write_msg(&[entry(4, b"", b"a"), entry(4, b"", b"b")]),
        Err(EmitError::Msg { id: 4, .. })
    ));
}

#[test]
fn a_msg_file_appends_to_a_base_without_touching_it() {
    let base = b"# a comment line without braces\n{1}{}{One}\n\n{2}{}{Two}  trailing text";
    let mut file = MsgFile::from_base(base).unwrap();
    assert_eq!(file.len(), 2);
    file.push(&entry(300, b"", b"Three hundred")).unwrap();
    assert!(file.push(&entry(2, b"", b"again")).is_err());
    let bytes = file.to_bytes();
    assert!(bytes.starts_with(base));
    let catalog = MsgCatalog::parse_strict(&bytes).unwrap();
    assert_eq!(catalog.get(1).unwrap().text, b"One");
    assert_eq!(catalog.get(2).unwrap().text, b"Two");
    assert_eq!(catalog.get(300).unwrap().text, b"Three hundred");
    assert!(MsgFile::from_base(b"{1}{}{unterminated").is_err());
}

#[test]
fn gam_files_round_trip_for_both_sections() {
    for section in [GamSection::Campaign, GamSection::Map] {
        let vars = vec![
            GamVar::new("FIRST", 0),
            GamVar::new("second var", -17),
            GamVar::new("MAX", i32::MAX),
            GamVar::new("MIN", i32::MIN),
            GamVar::new("dup", 1),
            GamVar::new("dup", 2),
            GamVar::new("/single/slash", 3),
        ];
        let bytes = LocalWriters.gam(section, &vars).unwrap();
        let file = GamFile::parse(&bytes, section).unwrap();
        assert!(file.found);
        assert_eq!(
            file.values,
            vars.iter().map(|var| var.value).collect::<Vec<_>>()
        );
        let empty = GamFile::parse(&write_gam(section, &[]).unwrap(), section).unwrap();
        assert!(empty.found && empty.values.is_empty());
    }
    // A map file is not a campaign file.
    let map = write_gam(GamSection::Map, &[GamVar::new("A", 1)]).unwrap();
    assert!(!GamFile::parse(&map, GamSection::Campaign).unwrap().found);
}

#[test]
fn gam_lines_fit_one_read() {
    let suffix = " = -2147483648;\n".len();
    let fits = GamVar::new("n".repeat(GAM_LINE_BYTES - suffix), i32::MIN);
    let bytes = write_gam(GamSection::Map, std::slice::from_ref(&fits)).unwrap();
    assert_eq!(
        GamFile::parse(&bytes, GamSection::Map).unwrap().values,
        [i32::MIN]
    );
    let over = GamVar::new("n".repeat(GAM_LINE_BYTES - suffix + 1), i32::MIN);
    assert!(matches!(
        write_gam(GamSection::Map, &[over]),
        Err(EmitError::Gam { index: 0, .. })
    ));
}

#[test]
fn the_gam_writer_refuses_names_the_parser_reads_as_syntax() {
    for name in [
        "",
        "a;b",
        "a=b",
        "//comment",
        "tab\there",
        "nl\n",
        "MAP_GLOBAL_VARS:x",
        "ünï",
    ] {
        assert!(
            matches!(
                write_gam(GamSection::Map, &[GamVar::new(name, 1)]),
                Err(EmitError::Gam { index: 0, .. })
            ),
            "{name:?}"
        );
    }
}
