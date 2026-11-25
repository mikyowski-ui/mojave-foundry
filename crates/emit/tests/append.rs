//! The appenders (`emit::append`): the base file's bytes kept, new rows
//! after them, numbers by key order, and the result read back by the
//! engine's parsers. Synthetic base files only.

use emit::append::{
    HolodiskRow, QuestRow, ScriptRow, append_globals, append_holodisks, append_message_entries,
    append_messages, append_quests, append_script_rows,
};
use emit::{EmitError, GamVar, MsgEntry};
use vault_formats::fallout2::lines;
use vault_formats::fallout2::text::Diagnostics;
use vault_formats::{GamFile, GamSection, LstKind, LstTable, MsgCatalog};

/// Every order of `items` (small inputs only).
fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for index in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(index);
        for mut tail in permutations(&rest) {
            tail.insert(0, head.clone());
            out.push(tail);
        }
    }
    out
}

// ---- Messages ----------------------------------------------------------------

const MSG_BASE: &[u8] =
    b"# a comment outside the braces\n{100}{}{One}\n{101}{snd}{Two}\ntrailing text";

#[test]
fn messages_get_ids_in_key_order_and_keep_the_base() {
    let rows = vec![
        ("c", b"third".to_vec()),
        ("a", b"first".to_vec()),
        ("b", b"second {with} no".to_vec()),
    ];
    // `}` cannot be carried: the writer refuses it, the adapter does not.
    assert!(matches!(
        append_messages(MSG_BASE, 100_000, &rows),
        Err(EmitError::Msg { id: 100_001, .. })
    ));
    let rows: Vec<(&str, Vec<u8>)> = rows
        .into_iter()
        .map(|(key, text)| {
            (
                key,
                text.into_iter()
                    .filter(|b| *b != b'{' && *b != b'}')
                    .collect(),
            )
        })
        .collect();
    let appended = append_messages(MSG_BASE, 100_000, &rows).unwrap();
    assert_eq!(
        appended.numbers,
        [("a", 100_000), ("b", 100_001), ("c", 100_002)]
    );
    // The base is first, byte for byte (one LF added after its last line).
    assert!(appended.bytes.starts_with(MSG_BASE));
    let catalog = MsgCatalog::parse_strict(&appended.bytes).unwrap();
    assert_eq!(catalog.get(100).unwrap().text, b"One");
    assert_eq!(catalog.get(101).unwrap().sound, b"snd");
    assert_eq!(catalog.get(100_000).unwrap().text, b"first");
    assert_eq!(catalog.get(100_001).unwrap().text, b"second with no");
    assert_eq!(catalog.get(100_002).unwrap().text, b"third");
    assert_eq!(catalog.entries().len(), 5);
    // Whatever the input order, the same bytes and numbers.
    for order in permutations(&rows) {
        assert_eq!(
            append_messages(MSG_BASE, 100_000, &order).unwrap(),
            appended
        );
    }
}

#[test]
fn message_appends_refuse_collisions_and_bad_input() {
    let row = |text: &str| vec![("k", text.as_bytes().to_vec())];
    // The base holds id 100.
    assert!(matches!(
        append_messages(MSG_BASE, 100, &row("x")),
        Err(EmitError::Msg { id: 100, .. })
    ));
    assert!(append_messages(b"{1}{}{unterminated", 5, &row("x")).is_err());
    assert!(append_messages(b"", i32::MAX, &[("a", vec![]), ("b", vec![])]).is_err());
    assert!(matches!(
        append_messages(b"", 1, &[("a", b"1".to_vec()), ("a", b"2".to_vec())]),
        Err(EmitError::Append(_))
    ));
    assert!(append_messages(b"", 1, &row("two\nlines")).is_err());
    // A new file.
    let fresh = append_messages(b"", 1, &[("x", b"hello".to_vec())]).unwrap();
    assert_eq!(fresh.bytes, b"{1}{}{hello}\n");

    // Explicit ids, in ascending order, refused when the base has one.
    let entries = [MsgEntry::new(10_021, "b"), MsgEntry::new(10_000, "a")];
    let bytes = append_message_entries(b"{5}{}{base}\n", &entries).unwrap();
    assert_eq!(bytes, b"{5}{}{base}\n{10000}{}{a}\n{10021}{}{b}\n");
    assert!(append_message_entries(b"{10000}{}{x}\n", &entries).is_err());
    assert!(append_message_entries(b"", &[MsgEntry::new(3, "a"), MsgEntry::new(3, "b")]).is_err());
}

// ---- Globals -----------------------------------------------------------------

#[test]
fn globals_continue_the_bases_numbering() {
    let base =
        b"GAME_GLOBAL_VARS:\n\n// a comment line\nGVAR_A = 5;\nGVAR_B = -3; trailing\nGVAR_C = 0;";
    let rows = vec![
        ("b", GamVar::new("MF_QUEST_B", 7)),
        ("a", GamVar::new("MF_QUEST_A", 1)),
        ("c", GamVar::new("MF_GLOB_C", -2)),
    ];
    let appended = append_globals(base, &rows).unwrap();
    assert_eq!(appended.numbers, [("a", 3), ("b", 4), ("c", 5)]);
    assert!(appended.bytes.starts_with(base));
    let parsed = GamFile::parse(&appended.bytes, GamSection::Campaign).unwrap();
    assert!(parsed.found);
    assert_eq!(parsed.values, [5, -3, 0, 1, 7, -2]);
    for order in permutations(&rows) {
        assert_eq!(append_globals(base, &order).unwrap(), appended);
    }
    // A base that ends in an LF, and the empty base.
    let neat = append_globals(b"GAME_GLOBAL_VARS:\nA = 9;\n", &rows).unwrap();
    assert_eq!(neat.numbers[0], ("a", 1));
    let empty = append_globals(b"", &rows).unwrap();
    assert_eq!(
        empty.bytes,
        b"GAME_GLOBAL_VARS:\nMF_QUEST_A = 1;\nMF_QUEST_B = 7;\nMF_GLOB_C = -2;\n"
    );
    assert_eq!(empty.numbers, [("a", 0), ("b", 1), ("c", 2)]);
    // No rows: the base stays whole.
    let none = append_globals::<&str>(b"GAME_GLOBAL_VARS:\nA = 9;\n", &[]).unwrap();
    assert_eq!(none.bytes, b"GAME_GLOBAL_VARS:\nA = 9;\n");
}

#[test]
fn globals_refuse_what_the_format_cannot_carry() {
    let row = |name: &str| vec![("k", GamVar::new(name, 1))];
    // A base without the section marker is not the campaign file.
    assert!(matches!(
        append_globals(b"MAP_GLOBAL_VARS:\nA = 1;\n", &row("X")),
        Err(EmitError::Append(_))
    ));
    for name in [
        "a;b",
        "a=b",
        "//c",
        "",
        "MAP_GLOBAL_VARS:x",
        "GAME_GLOBAL_VARS",
    ] {
        assert!(
            append_globals(b"GAME_GLOBAL_VARS:\n", &row(name)).is_err(),
            "{name:?}"
        );
    }
    // The error names the global's index in the whole file.
    let error = append_globals(b"GAME_GLOBAL_VARS:\nA = 1;\nB = 2;\n", &row("bad;")).unwrap_err();
    assert!(matches!(error, EmitError::Gam { index: 2, .. }), "{error}");
    assert!(
        append_globals(
            b"GAME_GLOBAL_VARS:\n",
            &[("a", GamVar::new("x", 1)), ("a", GamVar::new("y", 2))]
        )
        .is_err()
    );
}

// ---- scripts.lst ---------------------------------------------------------------

const LST_BASE: &[u8] = b"; the first row\r\nbase_a.int ; Base A # local_vars=3\r\n\r\nbase_b.int # local_vars=0\nlast.int";

fn script(stem: &str, editor: &str, locals: u32) -> ScriptRow {
    ScriptRow {
        stem: stem.into(),
        editor_id: Some(editor.into()),
        local_vars: locals,
    }
}

#[test]
fn script_rows_follow_the_base_list() {
    let base = LstTable::parse(LST_BASE, LstKind::Script).unwrap();
    let positions = base.entries().len();
    assert_eq!(positions, 5);
    let rows = vec![
        ("z", script("md000002", "TopicSpeaker2", 0)),
        ("a", script("mm000001", "MapScript1", 4)),
        (
            "m",
            ScriptRow {
                stem: "mo000001".into(),
                editor_id: None,
                local_vars: 12,
            },
        ),
    ];
    let appended = append_script_rows(LST_BASE, &rows).unwrap();
    assert_eq!(appended.numbers, [("a", 5), ("m", 6), ("z", 7)]);
    // The base is kept, with an LF where its last line had none.
    assert!(appended.bytes.starts_with(LST_BASE));
    assert_eq!(appended.bytes[LST_BASE.len()], b'\n');
    let back = LstTable::parse(&appended.bytes, LstKind::Script).unwrap();
    assert!(back.diagnostics().is_empty());
    assert_eq!(back.entries().len(), positions + 3);
    for (old, new) in base.entries().iter().zip(back.entries()) {
        // `raw` keeps each line's LF; only the last base line gained one.
        assert_eq!(
            old.raw.strip_suffix(b"\n").unwrap_or(&old.raw),
            new.raw.strip_suffix(b"\n").unwrap_or(&new.raw)
        );
        assert_eq!((&old.name, old.local_vars), (&new.name, new.local_vars));
    }
    let name = |index: usize| back.entries()[index].name.clone().unwrap();
    assert_eq!(name(5), b"mm000001.int");
    assert_eq!(name(6), b"mo000001.int");
    assert_eq!(name(7), b"md000002.int");
    assert_eq!(back.entries()[5].local_vars, Some(4));
    assert_eq!(back.entries()[6].local_vars, Some(12));
    assert_eq!(back.entries()[7].local_vars, Some(0));
    assert_eq!(
        back.entries()[5].raw,
        b"mm000001.int ; MapScript1 # local_vars=4\n"
    );
    for order in permutations(&rows) {
        assert_eq!(append_script_rows(LST_BASE, &order).unwrap(), appended);
    }
    // An empty base list: rows start at 0.
    let fresh = append_script_rows(b"", &rows[..1]).unwrap();
    assert_eq!(fresh.numbers, [("z", 0)]);
    assert_eq!(
        fresh.bytes,
        b"md000002.int ; TopicSpeaker2 # local_vars=0\n"
    );
}

#[test]
fn script_rows_refuse_bad_stems_and_repeats() {
    let one = |row: ScriptRow| vec![("k", row)];
    for stem in ["", "a.b", "with space", "x/y", "caf\u{e9}"] {
        assert!(
            append_script_rows(LST_BASE, &one(script(stem, "e", 0))).is_err(),
            "{stem:?}"
        );
    }
    assert!(append_script_rows(LST_BASE, &one(script("ok", "has # hash", 0))).is_err());
    assert!(append_script_rows(LST_BASE, &one(script("ok", "line\nbreak", 0))).is_err());
    assert!(append_script_rows(LST_BASE, &one(script("ok", "e", u32::MAX))).is_err());
    // A stem the base has (without case), or a new row repeats.
    assert!(append_script_rows(LST_BASE, &one(script("BASE_A", "e", 0))).is_err());
    assert!(
        append_script_rows(
            LST_BASE,
            &[("a", script("same", "e", 0)), ("b", script("SAME", "e", 0))]
        )
        .is_err()
    );
    // A line that would fill more than one bounded read.
    let long = "e".repeat(300);
    assert!(append_script_rows(LST_BASE, &one(script("ok", &long, 0))).is_err());
    assert!(matches!(
        append_script_rows(
            b"",
            &[("a", script("x", "e", 0)), ("a", script("y", "e", 0))]
        ),
        Err(EmitError::Append(_))
    ));
}

// ---- quests.txt and holodisk.txt ---------------------------------------------

#[test]
fn quests_and_holodisks_number_among_the_valid_lines() {
    let base = b"# location, description, gvar, display, completed\n1500, 100, 7, 1, 3\n1500, 101, 8, 2, 2 ; note\nbroken line\n1502, 103\n1501, 102, 9, 0, 5";
    let row = |location, description, global| QuestRow {
        location,
        description,
        global,
        display: 1,
        completed: 3,
    };
    let rows = vec![
        ("q2", row(100_001, 100_002, 301)),
        ("q1", row(100_000, 100_001, 300)),
    ];
    let appended = append_quests(base, &rows).unwrap();
    // Three valid base lines: the broken and the short line are dropped.
    assert_eq!(appended.numbers, [("q1", 3), ("q2", 4)]);
    assert!(appended.bytes.starts_with(base));
    let quests = lines::quests(&appended.bytes, &mut Diagnostics::default()).unwrap();
    assert_eq!(quests.len(), 5);
    assert_eq!(
        (quests[3].location, quests[3].description, quests[3].global),
        (100_000, 100_001, 300)
    );
    assert_eq!(quests[4].global, 301);
    assert_eq!((quests[4].display, quests[4].completed), (1, 3));
    assert_eq!(quests[2].global, 9, "the base's last line, which had no LF");
    for order in permutations(&rows) {
        assert_eq!(append_quests(base, &order).unwrap(), appended);
    }
    assert_eq!(
        append_quests(b"", &rows[1..]).unwrap().bytes,
        b"100000, 100001, 300, 1, 3\n"
    );

    let disks = vec![
        (
            "n2",
            HolodiskRow {
                global: 401,
                title: 100_001,
                text: 200_500,
            },
        ),
        (
            "n1",
            HolodiskRow {
                global: 400,
                title: 100_000,
                text: 200_000,
            },
        ),
    ];
    let disk_base = b"9, 1000, 5000\n";
    let appended = append_holodisks(disk_base, &disks).unwrap();
    assert_eq!(appended.numbers, [("n1", 1), ("n2", 2)]);
    assert_eq!(
        appended.bytes,
        b"9, 1000, 5000\n400, 100000, 200000\n401, 100001, 200500\n"
    );
    let read = lines::holodisks(&appended.bytes, &mut Diagnostics::default()).unwrap();
    assert_eq!(
        (read[1].global, read[1].title, read[1].text),
        (400, 100_000, 200_000)
    );
    for order in permutations(&disks) {
        assert_eq!(append_holodisks(disk_base, &order).unwrap(), appended);
    }
}

#[test]
fn line_tables_the_engine_would_refuse_are_refused() {
    let disk = HolodiskRow {
        global: 1,
        title: 2,
        text: 3,
    };
    assert!(append_holodisks(b"1, 2, 3\0\n", &[("a", disk)]).is_err());
    let long = format!("1, 2, 3 ; {}\n", "x".repeat(5000));
    assert!(append_holodisks(long.as_bytes(), &[("a", disk)]).is_err());
    assert!(matches!(
        append_holodisks(b"", &[("a", disk), ("a", disk)]),
        Err(EmitError::Append(_))
    ));
    // Negative numbers and extremes are written as the parser reads them.
    let extreme = HolodiskRow {
        global: i32::MIN,
        title: -1,
        text: i32::MAX,
    };
    let appended = append_holodisks(b"", &[("a", extreme)]).unwrap();
    let read = lines::holodisks(&appended.bytes, &mut Diagnostics::default()).unwrap();
    assert_eq!(
        (read[0].global, read[0].title, read[0].text),
        (i32::MIN, -1, i32::MAX)
    );
}
