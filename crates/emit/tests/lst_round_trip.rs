//! LST writer round trips through `LstTable::parse` for every kind.

use emit::lst::{ART_LINE_BYTES, PROTOTYPE_LINE_BYTES, SCRIPT_LINE_BYTES, write_lst};
use emit::{EmitError, FormatWriters, LocalWriters, LstLine};
use vault_formats::{LstKind, LstTable};

fn parse(lines: &[LstLine], kind: LstKind) -> LstTable {
    let bytes = LocalWriters.lst(lines).expect("writes");
    let table = LstTable::parse(&bytes, kind).expect("parses");
    assert_eq!(table.entries().len(), lines.len(), "one position per line");
    assert!(table.diagnostics().is_empty());
    table
}

#[test]
fn art_lines_keep_names_aliases_and_head_counts() {
    let lines = vec![
        LstLine::art("wall01.frm"),
        LstLine::Blank,
        LstLine::Art {
            name: "hmjmps".into(),
            numbers: vec![11],
        },
        LstLine::Art {
            name: "head".into(),
            numbers: vec![2, -3, 4],
        },
        LstLine::Art {
            name: "twelve.chars".into(),
            numbers: vec![i32::MAX, i32::MIN],
        },
    ];
    let table = parse(&lines, LstKind::Art);
    let e = table.entries();
    assert_eq!(e[0].name.as_deref(), Some(&b"wall01.frm"[..]));
    assert_eq!(e[0].alias, None);
    assert_eq!(e[1].name.as_deref(), Some(&b""[..]));
    assert_eq!(e[2].alias, Some(11));
    assert_eq!(e[2].head_columns, Some([11, 11, 11]));
    assert_eq!(e[3].alias, Some(2));
    assert_eq!(e[3].head_columns, Some([2, -3, 4]));
    assert_eq!(e[3].head_counts, Some([2, 2, 2]));
    assert_eq!(e[4].name.as_deref(), Some(&b"twelve.chars"[..]));
    assert_eq!(e[4].head_columns, Some([i32::MAX, i32::MIN, i32::MAX]));
    assert_eq!(table.art_id(4).unwrap().name, e[4].name);
}

#[test]
fn script_lines_keep_names_and_local_counts() {
    let lines = vec![
        LstLine::Script {
            stem: "obj_a".into(),
            local_vars: Some(12),
            comment: Some("a test script".into()),
        },
        LstLine::Script {
            stem: "b-2".into(),
            local_vars: None,
            comment: None,
        },
        LstLine::Blank,
        LstLine::Script {
            stem: "c".into(),
            local_vars: Some(0),
            comment: None,
        },
    ];
    let table = parse(&lines, LstKind::Script);
    let e = table.entries();
    assert_eq!(e[0].name.as_deref(), Some(&b"obj_a.int"[..]));
    assert_eq!(e[0].local_vars, Some(12));
    assert_eq!(e[1].name.as_deref(), Some(&b"b-2.int"[..]));
    assert_eq!(e[1].local_vars, None);
    assert_eq!(e[2].name, None);
    assert_eq!(e[3].local_vars, Some(0));
    assert_eq!(
        table.script_id(3).unwrap().name.as_deref(),
        Some(&b"c.int"[..])
    );
}

#[test]
fn prototype_lines_are_one_based() {
    let lines = vec![
        LstLine::prototype("00000001.pro"),
        LstLine::Prototype {
            file: "00000002.pro".into(),
            comment: Some("a comment, with; punctuation".into()),
        },
    ];
    let table = parse(&lines, LstKind::Prototype);
    assert_eq!(
        table.prototype_id(0x0200_0002).unwrap().name.as_deref(),
        Some(&b"00000002.pro"[..])
    );
    assert_eq!(
        table.prototype_id(0x0200_0001).unwrap().name.as_deref(),
        Some(&b"00000001.pro"[..])
    );
    assert!(table.prototype_id(0x0200_0000).is_none());
}

#[test]
fn raw_lines_are_kept_byte_for_byte() {
    let lines = vec![
        LstLine::Raw(b"any text, ; # here".to_vec()),
        LstLine::Raw(vec![0x80, 0xff, b' ']),
        LstLine::Blank,
    ];
    let table = parse(&lines, LstKind::Raw);
    assert_eq!(table.entries()[0].raw, b"any text, ; # here\n");
    assert_eq!(table.entries()[1].raw, [0x80, 0xff, b' ', b'\n']);
}

#[test]
fn empty_and_maximal_tables() {
    assert_eq!(write_lst(&[]).unwrap(), b"");
    assert!(
        LstTable::parse(b"", LstKind::Art)
            .unwrap()
            .entries()
            .is_empty()
    );
    let lines: Vec<_> = (0..4096)
        .map(|i| LstLine::art(format!("a{i:07}.frm")))
        .collect();
    let table = parse(&lines, LstKind::Art);
    assert_eq!(
        table.art_id(4095).unwrap().name.as_deref(),
        Some(&b"a0004095.frm"[..])
    );
}

#[test]
fn line_limits_are_the_parser_reads() {
    // A script line of exactly 259 bytes with its LF is one position.
    let prefix = "x.int ; ".len() + 1;
    let fits = LstLine::Script {
        stem: "x".into(),
        local_vars: None,
        comment: Some("c".repeat(SCRIPT_LINE_BYTES - prefix)),
    };
    parse(&[fits], LstKind::Script);
    let over = LstLine::Script {
        stem: "x".into(),
        local_vars: None,
        comment: Some("c".repeat(SCRIPT_LINE_BYTES - prefix + 1)),
    };
    assert!(matches!(
        write_lst(&[over]),
        Err(EmitError::Lst { line: 1, .. })
    ));
    let proto = LstLine::Prototype {
        file: "a.pro".into(),
        comment: Some("c".repeat(PROTOTYPE_LINE_BYTES - "a.pro ".len() - 1)),
    };
    parse(&[proto], LstKind::Prototype);
    let long_art = LstLine::Art {
        name: "a.frm".into(),
        numbers: vec![i32::MIN; 3],
    };
    assert!(write_lst(&[long_art]).unwrap().len() < ART_LINE_BYTES);
}

#[test]
fn the_writer_refuses_lines_that_would_read_differently() {
    let bad = [
        LstLine::art(""),
        LstLine::art("thirteen.char"),
        LstLine::art("a b.frm"),
        LstLine::art("a,b.frm"),
        LstLine::art("a;b"),
        LstLine::art("a\tb"),
        LstLine::art("a\rb"),
        LstLine::art("dir/a.frm"),
        LstLine::art("a?.frm"),
        LstLine::Art {
            name: "a".into(),
            numbers: vec![1, 2, 3, 4],
        },
        LstLine::Script {
            stem: "a.b".into(),
            local_vars: None,
            comment: None,
        },
        LstLine::Script {
            stem: " a".into(),
            local_vars: None,
            comment: None,
        },
        LstLine::Script {
            stem: "a".into(),
            local_vars: Some(-1),
            comment: None,
        },
        LstLine::Script {
            stem: "a".into(),
            local_vars: None,
            comment: Some("has # mark".into()),
        },
        LstLine::prototype("a b.pro"),
        LstLine::prototype(""),
        LstLine::Raw(b"two\nlines".to_vec()),
        LstLine::Raw(b"nul\0".to_vec()),
    ];
    for line in bad {
        assert!(
            matches!(
                write_lst(std::slice::from_ref(&line)),
                Err(EmitError::Lst { .. })
            ),
            "{line:?} should be refused"
        );
    }
    let mixed = [
        LstLine::art("a.frm"),
        LstLine::Blank,
        LstLine::prototype("1.pro"),
    ];
    assert!(matches!(
        write_lst(&mixed),
        Err(EmitError::Lst { line: 3, .. })
    ));
}
