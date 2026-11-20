# Owner brief: an LST writer in `vault-formats`

Basis: `vault-engine` at revision `3adeac9`, the `vault-formats` modules
`lst` (`LstTable::parse`, `LstKind`) and `fid` (`ArtCatalog`), and SPEC-001
§2.4 "`.LST` text and ID representation" and §3.6 "LST comments,
whitespace, metadata, and boundary cases"; SPEC-007 §2.1 "Complete 32-Bit Art Identifier"
for the 12-bit index field; SPEC-003 §2.1 for the one-based prototype
index.

Tags: `[Must-match]` the bytes and positions must be exactly as stated;
`[May-improve]` the parser's existing safe rule; `[Safe rule]` what the
writer does with input it cannot represent.

## 1. Purpose

A mod-authoring tool needs to add entries to the art lists
(`art/<category>/<category>.lst`), the prototype lists
(`proto/<category>/<category>.lst`) and the script list
(`scripts/scripts.lst`), and to write lists from scratch. Positions are the
identifiers: an art identifier's low 12 bits, a prototype identifier's
low 24 bits and a script index all select a list line. A writer must
therefore produce exactly one position per intended line, and keep every
existing line of a list it extends at its position, byte for byte. This
brief asks for that writer, and for an appending editor over a parsed list.

## 2. Proposed API

In the `lst` module:

```rust
/// One line to write, by the list kind it belongs to.
pub enum LstLine {
    /// name (1-12 bytes) then up to three numbers: `name,a,b,c`.
    Art { name: Vec<u8>, numbers: Vec<i32> },
    /// `stem.int`, optional ` ; comment`, optional ` # local_vars=N`.
    Script { stem: Vec<u8>, local_vars: Option<i32>, comment: Option<Vec<u8>> },
    /// file name, optional comment after one space.
    Prototype { file: Vec<u8>, comment: Option<Vec<u8>> },
    /// Any other list: the bytes of the line, without its LF.
    Raw(Vec<u8>),
    /// An empty line; it keeps a position in every kind.
    Blank,
}

/// Serialize as `LstTable::parse(_, kind)` reads it. Errors use
/// `ParseError` with format "LST" and the one-based line as offset.
pub fn write_lst(kind: LstKind, lines: &[LstLine]) -> Result<Vec<u8>, ParseError>;

impl LstTable {
    /// The list's bytes: every entry's `raw` bytes in order.
    pub fn write(&self) -> Vec<u8>;
    /// Append one line after the existing ones; returns its zero-based
    /// position. Existing lines are untouched.
    pub fn append(&mut self, line: &LstLine) -> Result<usize, ParseError>;
}
```

## 3. Layout, as the parser reads it

`[Must-match]` for the whole section.

- A position is one bounded line read: up to 199 bytes for art lists, 259
  for the script list, 255 for prototype lists, unbounded for others, the
  limit counting the returned LF. CRLF reads as LF; a bare CR is an
  ordinary byte. A final line without LF is a position; a final LF creates
  none; an empty file has none. Blank and comment-like lines keep their
  positions. (SPEC-001 §2.4.)
- Art name: the bytes before the first space, comma, semicolon, TAB or LF,
  kept to 12 bytes. The decimal number after the first comma is the critter
  alias; the first three comma-separated numbers are the head fidget counts
  under the SPEC-006 §3.11 safe rule (`LstEntry::head_columns`). Art index
  = position − 1. (SPEC-001 §2.4, §3.6.)
- Script name: the bytes before the first dot, plus `.int`; no dot, no
  name. The count after the literal `local_vars=` that follows the first
  `#` is the local-variable count. Script index = position − 1.
- Prototype name: the bytes before the first space or LF. Prototype
  index = position (the first line is 1).
- A line holding a NUL keeps its position with no name or metadata
  (the parser's safe rule, `LstTable::diagnostics`). `[May-improve]`

## 4. Validation and errors

`write_lst` and `append` refuse, writing nothing:

- a line containing LF, CR or NUL (it would split, merge, or lose its
  name); `[Safe rule]`
- an art name that is empty, over 12 bytes, or contains a space, comma,
  semicolon or TAB, or that is not a single canonical path component;
  `[Safe rule]`
- more than three art numbers; `[Safe rule]`
- a script stem that is empty or contains a dot or leading whitespace, a
  negative `local_vars`, or a comment containing `#`; `[Safe rule]`
- a prototype file that is empty or contains a space; `[Safe rule]`
- a line that, with its LF, exceeds the kind's read limit; `[Must-match]`
  (it would read as two positions)
- a line of another kind than `kind` (`Blank` fits every kind);
  `[Safe rule]`
- for art lists, an `append` that would create position 4097 or later: an
  art identifier addresses indexes 0 to 4095 (SPEC-007 §2.1). The error
  names the list, the limit and the count requested. `[Must-match]`

`append` on a list whose last line has no LF first adds the LF, unless that
line filled a whole read (then an LF would itself be a position).
`[Must-match]`

## 5. Round-trip property

For every accepted `lines`, `t = LstTable::parse(&write_lst(kind, &lines)?, kind)?`
has `t.entries().len() == lines.len()`, no diagnostics, and for each line
the entry's `name`, `alias`, `head_columns` and `local_vars` are those the
line describes (an art `Blank` has the empty name; a script `Blank` has
none). For every list `b` that parses, `parse(b)?.write() == b`, and after
`append`, `parse(write())` has the old entries unchanged at their positions
and the new one last. `[Must-match]`

## 6. Edge cases

- An empty list (no bytes).
- An art list of exactly 4096 entries; the 4097th append fails.
- A 12-byte art name; a 13-byte one is refused.
- Head lines with one, two and three numbers (`name,a` reads `[a, a, a]`,
  `name,a,b` reads `[a, b, a]`).
- A script line exactly 259 bytes long with its LF.
- A base list with CRLF lines, comment lines, duplicate names and no final
  LF, extended by `append`.

## 7. Synthetic test vectors

- Art, `[Art{"rock.frm"}, Blank, Art{"crit", [3]}, Art{"head", [1, 2, 3]}]`
  writes `rock.frm\n\ncrit,3\nhead,1,2,3\n`; entries: names `rock.frm`, ``,
  `crit`, `head`; aliases none, none, 3, 1; head columns of the last
  `[1, 2, 3]`.
- Script, `[Script{"obj_a", Some(12), Some("test")}]` writes
  `obj_a.int ; test # local_vars=12\n`; name `obj_a.int`, local vars 12.
- Prototype, `[Prototype{"00000001.pro"}, Prototype{"00000002.pro", Some("x")}]`
  writes `00000001.pro\n00000002.pro x\n`; `prototype_id(0x02000002)` names
  `00000002.pro`.
- Append to `a.frm` (no LF): the list becomes `a.frm\nb.frm\n`, `b.frm` at
  index 1. Append to 199 bytes of `a` with no LF: the list becomes those
  199 bytes then `b.frm\n`, `b.frm` at index 1.
- Refused: `Art{"a b"}`, `Art{"thirteen.char"}`, `Raw("x\ny")`,
  `Script{"a.b"}`, a 260-byte script line.

## 8. Out of scope

- Sorting, deduplication or any change to existing lines.
- How several mod layers' additions to one list are combined; that is an
  engine mounting question, not a file-writing one.
- Text encodings: names are raw bytes.
