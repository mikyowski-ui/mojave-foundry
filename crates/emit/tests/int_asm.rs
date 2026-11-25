//! The script assembler (`emit::int`): the brief's vectors A to C, the
//! loader's rules checked by an independent reader of the `.int` layout
//! (written from SPEC-004 sections 2.1 to 2.5 and the engine's
//! `IntProgram::parse_in`), and the error cases.

use emit::int::{self, AsmError, CORE_WORDS, Game, Words, assemble, flags};
use std::collections::BTreeMap;

// ---- An independent reader of the layout -----------------------------------

#[derive(Debug)]
struct Record {
    name_offset: u32,
    name: Vec<u8>,
    flags: u32,
    deadline: u32,
    condition: u32,
    body: u32,
    arguments: u32,
}

#[derive(Debug)]
struct Loaded {
    records: Vec<Record>,
    identifiers: BTreeMap<u32, Vec<u8>>,
    literals: BTreeMap<u32, Vec<u8>>,
    code_start: usize,
    boundaries: Vec<bool>,
}

fn word(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes
        .get(at..at + 4)
        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
        .ok_or_else(|| format!("truncated word at {at:#x}"))
}

/// The string table at `origin`, the offsets of its texts and where the
/// next section starts.
fn table(
    bytes: &[u8],
    origin: usize,
    may_be_absent: bool,
) -> Result<(BTreeMap<u32, Vec<u8>>, usize), String> {
    if may_be_absent && word(bytes, origin)? == u32::MAX {
        return Ok((BTreeMap::new(), origin + 4));
    }
    let length = word(bytes, origin)? as i32;
    let length = usize::try_from(length).map_err(|_| "negative table length".to_string())?;
    let end = origin + 4 + length;
    if end > bytes.len() {
        return Err("table exceeds the image".into());
    }
    let mut at = origin + 4;
    let mut out = BTreeMap::new();
    while at < end {
        if end - at < 2 {
            return Err("truncated string length".into());
        }
        let size = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
        at += 2;
        if size == 0 || size > end - at {
            return Err("invalid string extent".into());
        }
        let data = &bytes[at..at + size];
        let nul = data
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("string is not NUL terminated")?;
        out.insert((at - origin) as u32, data[..nul].to_vec());
        at += size;
    }
    if word(bytes, end)? != u32::MAX {
        return Err("missing table terminator".into());
    }
    Ok((out, end + 4))
}

fn load(bytes: &[u8], words: &Words) -> Result<Loaded, String> {
    if bytes.len() > int::MAX_IMAGE_BYTES {
        return Err("over 64 MiB".into());
    }
    let count = word(bytes, 42)? as usize;
    if count > 65_536 {
        return Err("too many procedures".into());
    }
    let identifier_origin = 46 + count * 24;
    let (identifiers, literal_origin) = table(bytes, identifier_origin, false)?;
    let (literals, code_start) = table(bytes, literal_origin, true)?;
    let mut records = Vec::new();
    for index in 0..count {
        let at = 46 + 24 * index;
        let name_offset = word(bytes, at)?;
        let name = identifiers
            .get(&name_offset)
            .ok_or("name reference is not a string start")?
            .clone();
        let arguments = word(bytes, at + 20)?;
        if arguments > 341 {
            return Err("argument count over 341".into());
        }
        records.push(Record {
            name_offset,
            name,
            flags: word(bytes, at + 4)?,
            deadline: word(bytes, at + 8)?,
            condition: word(bytes, at + 12)?,
            body: word(bytes, at + 16)?,
            arguments,
        });
    }
    let mut boundaries = vec![false; bytes.len()];
    for (start, end) in [(0, 42), (code_start, bytes.len())] {
        let mut at = start;
        while at < end {
            if end - at < 2 {
                return Err(format!("truncated instruction at {at:#x}"));
            }
            let op = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
            if !words.accepts(op) {
                return Err(format!("invalid instruction {op:#06x} at {at:#x}"));
            }
            let size = if op & 0x3ff == 1 { 6 } else { 2 };
            if end - at < size {
                return Err("truncated immediate".into());
            }
            boundaries[at] = true;
            at += size;
        }
    }
    for record in &records {
        // The body is always checked; a condition of 0 is none.
        for (address, checked) in [
            (record.body, true),
            (record.condition, record.condition != 0),
        ] {
            let at = address as usize;
            if checked && at != bytes.len() && !boundaries.get(at).copied().unwrap_or(false) {
                return Err(format!("address {at:#x} is not an instruction boundary"));
            }
        }
    }
    Ok(Loaded {
        records,
        identifiers,
        literals,
        code_start,
        boundaries,
    })
}

fn asm(source: &str) -> Vec<u8> {
    assemble(source, &Words::FALLOUT2).unwrap_or_else(|error| panic!("{error}\n{source}"))
}

fn error(source: &str) -> AsmError {
    assemble(source, &Words::FALLOUT2).expect_err("the source should be refused")
}

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect()
}

// ---- The brief's vectors ---------------------------------------------------

const VECTOR_A: &str = include_str!("int_vector_a.asm");

/// SPEC-004 section 4.1, the 282-byte fixture.
const FIXTURE_A: &str = "
80 02 C0 01 00 00 00 12 80 0D C0 01 00 00 00 84
80 04 80 10 80 1A 80 20 80 1A 80 21 80 1A 80 22
80 1A 80 23 80 24 80 25 80 26 00 00 00 02 00 00
00 06 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00 9C 00 00 00 00 00 00 00 0E 00 00 00 00 00 00
00 00 00 00 00 00 00 00 00 FC 00 00 00 02 00 00
00 10 00 06 6D 61 69 6E 00 00 00 06 73 75 6D 78
79 00 FF FF FF FF 00 00 00 06 00 04 68 69 00 00
FF FF FF FF 80 2C C0 01 00 00 00 00 C0 01 00 00
00 00 80 03 C0 01 00 00 00 9C 80 04 80 2B C0 01
00 00 00 C0 80 0D C0 01 00 00 00 05 C0 01 00 00
00 07 C0 01 00 00 00 02 C0 01 00 00 00 01 80 05
C0 01 00 00 00 E8 80 18 C0 01 00 00 00 0A 80 38
80 2F C0 01 00 00 00 63 C0 01 00 00 00 00 80 13
C0 01 00 00 00 F6 80 04 C0 01 FF FF FF FF C0 01
00 00 00 00 80 13 80 2A 80 29 80 1C 80 2B C0 01
00 00 00 00 80 32 C0 01 00 00 00 01 80 32 80 39
80 0D 80 2A 80 0C 80 29 80 1C
";

#[test]
fn vector_a_is_the_spec_fixture_byte_for_byte() {
    let bytes = asm(VECTOR_A);
    let fixture = hex(FIXTURE_A);
    assert_eq!(fixture.len(), 282);
    assert_eq!(bytes, fixture);

    let loaded = load(&bytes, &Words::FALLOUT2).unwrap();
    assert_eq!(loaded.code_start, 0x84);
    assert_eq!(loaded.records.len(), 2);
    let (main, sumxy) = (&loaded.records[0], &loaded.records[1]);
    assert_eq!((main.name.as_slice(), main.name_offset), (&b"main"[..], 6));
    assert_eq!(
        (sumxy.name.as_slice(), sumxy.name_offset),
        (&b"sumxy"[..], 14)
    );
    assert_eq!(
        (main.body, main.arguments, sumxy.body, sumxy.arguments),
        (0x9c, 0, 0xfc, 2)
    );
    assert_eq!(loaded.literals.get(&6).map(Vec::as_slice), Some(&b"hi"[..]));
    // Every assembled instruction address is a boundary.
    assert!(loaded.boundaries[0x84] && loaded.boundaries[0x11a - 2]);
}

#[test]
fn vector_b_has_no_literal_table() {
    let source = "
.procedure start
.entry init
init:
    push.i 0
    critical_off
    push.a start
    jmp
start:
    frame
    drop_frame
    restore_base
    ret
";
    let bytes = asm(source);
    let loaded = load(&bytes, &Words::FALLOUT2).unwrap();
    // N is 1, I is 0x46, the identifier payload is `00 06 start 00`.
    assert_eq!(&bytes[42..46], &[0, 0, 0, 1]);
    assert_eq!(&bytes[0x46..0x4a], &[0, 0, 0, 8]);
    assert_eq!(&bytes[0x4a..0x52], b"\x00\x06start\x00");
    assert_eq!(&bytes[0x52..0x56], &[0xff; 4]);
    // The literal table word is at T = 0x56, the code starts at 0x5a, where
    // the bootstrap's jump goes.
    assert_eq!(&bytes[0x56..0x5a], &[0xff; 4]);
    assert_eq!(loaded.code_start, 0x5a);
    assert_eq!(&bytes[0x0a..0x10], &[0xc0, 0x01, 0, 0, 0, 0x5a]);
    assert!(loaded.literals.is_empty());
    // `end` is at offset 18, the trampoline the root returns to.
    assert_eq!(&bytes[18..20], &[0x80, 0x10]);
    // The body of `start`: frame, drop_frame, restore_base, ret.
    assert_eq!(
        &bytes[bytes.len() - 8..],
        &[0x80, 0x2b, 0x80, 0x2a, 0x80, 0x29, 0x80, 0x1c]
    );
}

#[test]
fn vector_c_is_refused() {
    // A host word outside the Fallout 1 set, for that set: `metarule3` is a
    // Fallout 2 name (0x80e1), and 0x8156 is beyond the Fallout 1 range.
    let for_fallout1 = |source: &str| assemble(source, &Words::FALLOUT1).unwrap_err();
    let host = ".procedure s\n.entry s\ns:\n    inven_cmds\n";
    let refused = for_fallout1(host);
    assert_eq!(refused.line, 4, "{refused}");
    assert!(for_fallout1(".entry s\ns:\n    op 0x8156\n").line == 3);
    assert!(assemble(host, &Words::FALLOUT2).is_ok());

    let nowhere = error(".procedure s\n.entry s\ns:\n    push.a nowhere\n");
    assert_eq!(nowhere.line, 4);
    assert!(nowhere.message.contains("nowhere"));

    let args = error(".procedure x args=342\n.entry x\nx:\n    nop\n");
    assert_eq!(args.line, 1);
    assert!(
        assemble(
            ".procedure x args=341\n.entry x\nx:\n    nop\n",
            &Words::FALLOUT2
        )
        .is_ok()
    );
}

// ---- The round trip ----------------------------------------------------------

#[test]
fn procedures_names_literals_and_boundaries_round_trip() {
    let source = r#"
.procedure start
.procedure talk_p_proc flags=critical
.procedure timed_one flags=timed deadline=60 args=1
.procedure cond_one flags=conditional condition=check args=3
.import imported_one args=2
.name my_var
.literal one "hello \"you\""
.literal two "tab\x09end \\"
.literal three ""
.literal again "hello \"you\""
.entry init
init:
    push.i 0
    critical_off
    push.a start
    jmp
start:
    frame
    push.s one
    push.s again
    push.s two
    push.s three
    push.n my_var
    export_var
    drop_frame
    restore_base
    ret
talk_p_proc:
    ret
timed_one:
    ret
cond_one:
    ret
check:
    push.i 1
    ret
"#;
    let bytes = asm(source);
    let loaded = load(&bytes, &Words::FALLOUT2).unwrap();
    let names: Vec<&[u8]> = loaded.records.iter().map(|r| r.name.as_slice()).collect();
    assert_eq!(
        names,
        [
            &b"start"[..],
            b"talk_p_proc",
            b"timed_one",
            b"cond_one",
            b"imported_one"
        ]
    );
    let by_name = |name: &[u8]| loaded.records.iter().find(|r| r.name == name).unwrap();
    assert_eq!(by_name(b"talk_p_proc").flags, flags::CRITICAL);
    let timed = by_name(b"timed_one");
    assert_eq!((timed.flags, timed.deadline, timed.arguments), (1, 60, 1));
    let cond = by_name(b"cond_one");
    assert_eq!((cond.flags, cond.arguments), (2, 3));
    assert!(loaded.boundaries[cond.condition as usize]);
    assert_ne!(cond.condition, cond.body);
    let import = by_name(b"imported_one");
    assert_eq!(
        (import.flags, import.body, import.arguments),
        (flags::IMPORT, 0, 2)
    );
    // Literals: the repeated text shares a record; the empty text keeps one.
    let texts: Vec<&[u8]> = loaded.literals.values().map(Vec::as_slice).collect();
    assert_eq!(texts, [&b"hello \"you\""[..], b"tab\tend \\", b""]);
    // `my_var` is an identifier record of its own (no procedure).
    assert!(loaded.identifiers.values().any(|name| name == b"my_var"));
    // Each push operand addresses the declared string.
    let code = &bytes[loaded.code_start..];
    let pushes: Vec<(u16, u32)> = code
        .chunks(1)
        .enumerate()
        .filter_map(|(at, _)| {
            let op = u16::from_be_bytes([*code.get(at)?, *code.get(at + 1)?]);
            let is_start = loaded.boundaries[loaded.code_start + at];
            (is_start && op & 0x3ff == 1).then(|| (op, word(code, at + 2).unwrap()))
        })
        .collect();
    let strings: Vec<u32> = pushes
        .iter()
        .filter(|(tag, _)| *tag == 0x9001)
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(strings.len(), 4);
    assert_eq!(strings[0], strings[1], "equal texts share one offset");
    for offset in &strings {
        assert!(loaded.literals.contains_key(offset));
    }
    let name_push = pushes.iter().find(|(tag, value)| {
        *tag == 0xc001
            && loaded
                .identifiers
                .get(value)
                .is_some_and(|n| n == b"my_var")
    });
    assert!(name_push.is_some());
}

#[test]
fn push_forms_and_raw_words() {
    let bytes = asm("
.entry e
e:
    push.i 0x7fffffff
    push.i -2147483648
    push.i 0xFFFFFFFF
    push.f 1.5
    push.f -0.25
    push.raw 0x9801 7
    push.raw 0xFC01 0xdeadbeef
    op 0x8049
");
    let loaded = load(&bytes, &Words::FALLOUT2).unwrap();
    let code = &bytes[loaded.code_start..];
    let expected: Vec<u8> = [
        [0xc0, 0x01, 0x7f, 0xff, 0xff, 0xff],
        [0xc0, 0x01, 0x80, 0, 0, 0],
        [0xc0, 0x01, 0xff, 0xff, 0xff, 0xff],
        [0xa0, 0x01, 0x3f, 0xc0, 0, 0],
        [0xa0, 0x01, 0xbe, 0x80, 0, 0],
        [0x98, 0x01, 0, 0, 0, 7],
        [0xfc, 0x01, 0xde, 0xad, 0xbe, 0xef],
    ]
    .concat();
    assert_eq!(&code[..expected.len()], expected);
    assert_eq!(&code[expected.len()..], &[0x80, 0x49]);
}

#[test]
fn every_core_and_host_mnemonic_assembles_to_its_word() {
    for words in [
        Words::FALLOUT1,
        Words::FALLOUT2,
        Words::FALLOUT2.with_mods(true),
    ] {
        for (name, word) in CORE_WORDS {
            let bytes = assemble(&format!(".entry e\ne:\n    {name}\n"), &words).unwrap();
            assert_eq!(&bytes[bytes.len() - 2..], word.to_be_bytes(), "{name}");
        }
        let mut host = 0;
        for word in 0x804c..=0x8183u16 {
            if let Some(name) = words.name_of(word) {
                assert!(words.accepts(word), "{name}");
                assert_eq!(words.word_named(name), Some(word), "{name} round trips");
                let source = format!(".entry e\ne:\n    {name}\n");
                let bytes = assemble(&source, &words).unwrap();
                assert_eq!(&bytes[bytes.len() - 2..], word.to_be_bytes(), "{name}");
                host += 1;
            }
        }
        assert!(host > 200, "{host} host mnemonics");
    }
    // The Fallout 2 renames replace the Fallout 1 names of their words.
    assert_eq!(Words::FALLOUT2.word_named("mark_area_known"), Some(0x80b2));
    assert_eq!(Words::FALLOUT2.word_named("reaction_roll"), None);
    assert_eq!(Words::FALLOUT1.word_named("reaction_roll"), Some(0x80b2));
    assert_eq!(Words::FALLOUT1.word_named("mark_area_known"), None);
    assert_eq!(Words::FALLOUT2.word_named("town_map"), None);
    assert_eq!(Words::FALLOUT1.word_named("town_map"), Some(0x8109));
    // Manifest scripting words and the enhanced range need their flag.
    assert_eq!(Words::FALLOUT2.word_named("mod_log"), None);
    assert_eq!(
        Words::FALLOUT2.with_mods(true).word_named("mod_log"),
        Some(0x8303)
    );
    assert!(!Words::FALLOUT2.accepts(0x8303));
    assert!(Words::FALLOUT2.with_mods(true).accepts(0x8309));
    assert!(!Words::FALLOUT2.with_mods(true).accepts(0x830a));
    assert!(Words::FALLOUT2.with_enhanced(true).accepts(0x8213));
    assert!(!Words::FALLOUT2.accepts(0x8213));
    assert_eq!(Words::FALLOUT2.game, Game::Fallout2);
}

#[test]
fn no_two_mnemonics_share_a_name_or_a_word_within_one_set() {
    for words in [
        Words::FALLOUT1,
        Words::FALLOUT2.with_mods(true).with_enhanced(true),
    ] {
        let mut seen = BTreeMap::new();
        for word in 0x8000..=0x8309u16 {
            if let Some(name) = words.name_of(word)
                && let Some(other) = seen.insert(name, word)
            {
                panic!("{name} names {other:#06x} and {word:#06x}");
            }
        }
    }
}

#[test]
fn the_host_words_of_a_set_load() {
    let source = "
.entry e
e:
    push.i 1
    reg_anim_turn_towards
    mark_area_known
    sayquit
";
    let bytes = assemble(source, &Words::FALLOUT2).unwrap();
    load(&bytes, &Words::FALLOUT2).unwrap();
    // The loader of the Fallout 1 set refuses the Fallout 2 additions.
    assert!(load(&bytes, &Words::FALLOUT1).is_err());
}

// ---- Errors --------------------------------------------------------------------

#[test]
fn errors_name_their_line() {
    let cases: &[(&str, usize, &str)] = &[
        (".entry e\ne:\n    frobnicate\n", 3, "unknown word"),
        (".entry e\ne:\n    push.i abc\n", 3, "not a number"),
        (".entry e\ne:\n    push.i 4294967296\n", 3, "32-bit"),
        (".entry e\ne:\n    push.i 0xFFFFFFFFF\n", 3, "32-bit"),
        (".entry e\ne:\n    push.i 3000000000\n", 3, "32-bit"),
        (".entry e\ne:\n    push.i\n", 3, "operand"),
        (".entry e\ne:\n    nop 1\n", 3, "operand"),
        (".entry e\ne:\n    push.f nan\n", 3, "finite"),
        (".entry e\ne:\n    push.f inf\n", 3, "finite"),
        (".entry e\ne:\n    push.f x\n", 3, "decimal"),
        (".entry e\ne:\n    push.s nothing\n", 3, "undefined literal"),
        (
            ".entry e\ne:\n    push.n nothing\n",
            3,
            "not a declared name",
        ),
        (
            ".entry e\ne:\n    push.p nothing\n",
            3,
            "undefined procedure",
        ),
        (".entry e\ne:\n    push.raw 0xC002 1\n", 3, "push word"),
        (".entry e\ne:\n    push.raw 0x4001 1\n", 3, "push word"),
        (".entry e\ne:\n    op 0x8001\n", 3, "push.raw"),
        (".entry e\ne:\n    op 0x8184\n", 3, "not accepted"),
        (".entry e\ne:\n    op 0x7fff\n", 3, "not accepted"),
        (".entry e\ne:\n    op 0x10000\n", 3, "outside"),
        (
            ".entry e\ne:\n    nop\ne:\n    nop\n",
            4,
            "already defined at line 2",
        ),
        (".entry e\ne:\n    nop\n.frob x\n", 4, "unknown directive"),
        (".entry e\n.entry e\ne:\n    nop\n", 2, "already given"),
        (
            ".procedure a\n.procedure a\n.entry a\na:\n",
            2,
            "declared twice",
        ),
        (".procedure a bogus\n.entry a\na:\n", 1, "key=value"),
        (
            ".procedure a color=red\n.entry a\na:\n",
            1,
            "unknown option",
        ),
        (".procedure a args=x\n.entry a\na:\n", 1, "not a number"),
        (
            ".procedure a flags=sparkly\n.entry a\na:\n",
            1,
            "not a number",
        ),
        (
            ".procedure a condition=nowhere\n.entry a\na:\n",
            1,
            "undefined label",
        ),
        (".procedure a\n.entry a\n", 2, "undefined label"),
        (".procedure a b\n.entry a\na:\n", 1, "key=value"),
        (".entry nowhere\n", 1, "undefined label"),
        ("e:\n    nop\n", 0, ".entry"),
        (".entry e\ne:\n    nop\n    \"text\"\n", 4, "quoted text"),
        (".entry e\n.literal x \"open\n", 2, "not closed"),
        (".entry e\n.literal x \"bad \\q\"\n", 2, "unknown escape"),
        (".entry e\n.literal x \"bad \\x4\"\n", 2, "two hex digits"),
        (".entry e\n.literal x \"nul \\x00\"\n", 2, "NUL"),
        (".entry e\n.literal x \"caf\u{e9}\"\n", 2, "ASCII"),
        (".entry e\n.literal x\n", 2, "quoted text"),
        (".entry e\n.literal x \"a\" \"b\"\n", 2, "one quoted text"),
        (
            ".entry e\n.literal x \"a\"\n.literal x \"b\"\n",
            3,
            "already defined at line 2",
        ),
        (".entry e\n.name a b\n", 2, "one name"),
        (".entry e\n.name a=b\n", 2, "printable"),
        (".entry e\n.import a flags=3\n", 2, "unknown option"),
        (".entry e\n:\ne:\n", 2, "label"),
    ];
    for (source, line, fragment) in cases {
        let error = assemble(source, &Words::FALLOUT2).expect_err(&format!("accepted: {source:?}"));
        assert_eq!(error.line, *line, "{source:?}: {error}");
        assert!(
            error.message.contains(fragment),
            "{source:?}: {error} lacks {fragment:?}"
        );
        assert_eq!(
            error.to_string(),
            format!("line {}: {}", error.line, error.message)
        );
    }
}

#[test]
fn table_limits_are_refused_not_panicked() {
    // The longest text a record holds, and one more.
    let longest = "a".repeat(int::MAX_STRING_BYTES);
    let ok = format!(".literal x \"{longest}\"\n.entry e\ne:\n    nop\n");
    let bytes = assemble(&ok, &Words::FALLOUT2).unwrap();
    load(&bytes, &Words::FALLOUT2).unwrap();
    let too_long = format!(".literal x \"{longest}a\"\n.entry e\ne:\n    nop\n");
    let refused = assemble(&too_long, &Words::FALLOUT2).unwrap_err();
    assert_eq!(refused.line, 1);

    // Identifiers share the limit.
    let name = "n".repeat(int::MAX_STRING_BYTES + 1);
    let refused = assemble(
        &format!(".name {name}\n.entry e\ne:\n    nop\n"),
        &Words::FALLOUT2,
    );
    assert!(refused.is_err());

    // 65,536 procedures load; one more is refused.
    let mut source = String::new();
    for index in 0..65_536 {
        source += &format!(".procedure p{index} body=e\n");
    }
    source += ".entry e\ne:\n    nop\n";
    let bytes = assemble(&source, &Words::FALLOUT2).unwrap();
    assert_eq!(
        load(&bytes, &Words::FALLOUT2).unwrap().records.len(),
        65_536
    );
    let refused = assemble(
        &format!(".procedure extra body=e\n{source}"),
        &Words::FALLOUT2,
    )
    .unwrap_err();
    assert_eq!(refused.line, 65_537, "{refused}");
}

#[test]
fn arbitrary_text_never_panics() {
    // A deterministic sweep of garbage: any outcome but a panic is fine.
    let pieces = [
        ".procedure",
        ".literal",
        ".entry",
        ".name",
        ".import",
        "push.i",
        "push.s",
        "push.a",
        "push.raw",
        "op",
        "nop",
        "a:",
        "\"",
        "\\",
        "\\x",
        ";",
        "=",
        "args=3",
        "flags=timed|x",
        "condition=a",
        "0x",
        "-",
        "99999999999999999999",
        "\u{e9}",
        "\t",
        "a",
        "b",
    ];
    let mut state = 0x9e37_79b9_u32;
    let mut next = move |bound: usize| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as usize % bound
    };
    for _ in 0..2_000 {
        let mut source = String::new();
        for _ in 0..next(12) {
            for _ in 0..next(5) {
                source += pieces[next(pieces.len())];
                source += if next(3) == 0 { "" } else { " " };
            }
            source.push('\n');
        }
        for words in [Words::FALLOUT1, Words::FALLOUT2] {
            if let Ok(bytes) = assemble(&source, &words) {
                load(&bytes, &words).unwrap_or_else(|error| panic!("{error}: {source:?}"));
            }
        }
    }
}

#[test]
fn output_is_deterministic_and_comments_do_not_matter() {
    let plain = asm(VECTOR_A);
    assert_eq!(plain, asm(VECTOR_A));
    let noisy = VECTOR_A
        .lines()
        .map(|line| format!("  {line}   ; a comment ; with a semicolon"))
        .collect::<Vec<_>>()
        .join("\n\n");
    assert_eq!(asm(&noisy), plain);
    // A `;` inside a quoted literal is text, not a comment.
    let semicolon = asm(".literal s \"a;b\"\n.entry e\ne: nop\n");
    let loaded = load(&semicolon, &Words::FALLOUT2).unwrap();
    assert_eq!(loaded.literals.values().next().unwrap(), b"a;b");
    // A label may share its line with an instruction.
    assert_eq!(semicolon[semicolon.len() - 2..], [0x80, 0x00]);
}

#[test]
fn procedure_order_and_shared_bodies() {
    // Record 0 shares `start`'s body, as the corpus does.
    let source = "
.procedure root body=start
.procedure start
.entry init
init:
    push.i 0
    critical_off
    push.a start
    jmp
start:
    frame
    ret
";
    let bytes = asm(source);
    let loaded = load(&bytes, &Words::FALLOUT2).unwrap();
    assert_eq!(loaded.records[0].body, loaded.records[1].body);
    assert_eq!(loaded.records[0].name, b"root");
    // Names keep first-declaration order whatever the code order.
    let (first, second) = (&loaded.records[0], &loaded.records[1]);
    assert!(first.name_offset < second.name_offset);
}
