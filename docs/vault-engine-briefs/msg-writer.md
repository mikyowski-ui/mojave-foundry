# Owner brief: a MSG writer in `vault-formats`

Basis: `vault-engine` at revision `3adeac9`, the `vault-formats` module
`msg` (`MsgCatalog::parse_strict`, `MsgCatalog::load`, `Message`) and
SPEC-002 §2.5 "`.MSG`: Byte-Oriented Brace Fields", §3.4 "`.MSG` Scanning,
Multiline Text, and Comments", §3.5 "`.MSG` IDs, Replacement, Errors, and
Limits" and §3.6 "`.MSG` Encoding and Display".

Tags: `[Must-match]` the bytes must be exactly as stated; `[May-improve]`
the parser's existing safe rule; `[Safe rule]` what the writer does with
input it cannot represent.

## 1. Purpose

A mod-authoring tool needs to write message files: object names and
descriptions, dialogue and interface text. The format has no escape
language: a closing brace always ends a field, line feeds inside a field
are dropped, and a NUL cuts the stored text. A writer must therefore refuse
the text it cannot carry, never write something that loads as different
text, and be able to extend an existing file without disturbing it. This
brief asks for that writer.

## 2. Proposed API

In the `msg` module:

```rust
impl MsgCatalog {
    /// Every record, ascending by ID, one `{id}{sound}{text}` line each.
    /// Errors use `ParseError` with format "MSG" and the ID as offset.
    pub fn write(&self) -> Result<Vec<u8>, ParseError>;
    /// Insert or replace a record after checking it can be written.
    pub fn insert(&mut self, id: i32, message: Message) -> Result<(), ParseError>;
}

/// One record's bytes: `{id}{sound}{text}` and an LF.
pub fn write_record(id: i32, message: &Message) -> Result<Vec<u8>, ParseError>;

/// `base` (which must load with `parse_strict`) followed by new records,
/// so its own bytes, comments and order are kept. A new ID the base
/// already holds is refused.
pub fn append_records(base: &[u8], records: &[(i32, Message)]) -> Result<Vec<u8>, ParseError>;
```

## 3. Layout, as the parser reads it

`[Must-match]` for the whole section.

- Outside a field the scanner skips every byte until `{`; a `}` there is
  a delimiter error (SPEC-002 §3.4).
- Inside a field every byte through the first `}` is kept, except LF; the
  LF of a CRLF is dropped with it; a bare CR is kept. An opening brace
  inside a field is literal. There is no escape (SPEC-002 §3.4).
- A record is three fields in order: ID, sound, text. End of input before
  an ID ends the load successfully; before a sound or text field it fails
  (SPEC-002 §3.5).
- An ID is an optional sign and decimal digits, untrimmed, within the
  signed 32-bit range; a sign alone is ID 0 (SPEC-002 §3.5;
  `[May-improve]` for the range check).
- A later record with the same ID replaces the earlier one (sound and text
  both).
- A field keeps at most 1023 bytes; a 1024th byte fails the load
  (`[May-improve]` in SPEC-002 §3.5).
- Bytes are raw; nothing is decoded (SPEC-002 §3.6).

The writer emits `{`, the ID in decimal (`-` for negatives, no `+`, no
leading zeroes), `}{`, the sound bytes, `}{`, the text bytes, `}` and LF.
`[Must-match]`

## 4. Validation and errors

A record is refused, and nothing written, when its sound or text:

- contains `}` (it would end the field early); `[Safe rule]`
- contains LF (it would be dropped); `[Safe rule]`
- contains NUL (the stored text would stop there); `[Safe rule]`
- contains `0x1A` (an end of file for loose text on the reference
  runtime, SPEC-002 §3.6); `[Safe rule]`
- holds more than 1023 bytes. `[Must-match]`

A bare CR is allowed: the writer never puts an LF after it inside a field,
so the parser keeps it. `[Must-match]` `append_records` refuses an ID the
base already holds, and a list with one ID twice, since a replacement on
load would be silent. `[Safe rule]` The total stays below the parser's
one-million record limit, counting the scan step for the final LF.
`[Safe rule]`

## 5. Round-trip property

For every catalog `c` whose records pass §4,
`MsgCatalog::parse_strict(&c.write()?)? == c` (same IDs, sound and text),
and `MsgCatalog::parse(&c.write()?)?.failure()` is `None`. For
`append_records(base, r)`, the output starts with `base` (plus at most one
LF when `base` has no final LF) and loads as the base's records plus `r`.
`[Must-match]`

## 6. Edge cases

- An empty catalog writes zero bytes, which loads as an empty catalog.
- IDs `i32::MIN`, `-1`, `0` and `i32::MAX`.
- Empty sound and text fields.
- Text with `{`, `#`, `;` and `//`, which are literal inside a field.
- Text with bytes `0x80`-`0xFF`.
- A 1023-byte text (accepted) and a 1024-byte one (refused).
- A base whose last record is followed by trailing non-brace bytes and no
  LF.

## 7. Synthetic test vectors

- `{(3, "", "c"), (-1, "", "a"), (2, "", "b")}` writes
  `{-1}{}{a}\n{2}{}{b}\n{3}{}{c}\n`.
- `(100, "snd01", "x { y")` writes `{100}{snd01}{x { y}\n` and reads back
  sound `snd01`, text `x { y`.
- `(5, "", "a\rb")` writes the bytes `7B 35 7D 7B 7D 7B 61 0D 62 7D 0A`
  and reads back text `61 0D 62`.
- `append_records("{1}{}{One}", [(2, "", "Two")])` gives
  `{1}{}{One}\n{2}{}{Two}\n`.
- Refused: text `a}b`, text `a\nb`, text `a\0b`, a 1024-byte sound,
  appending ID 1 to the base above.

## 8. Out of scope

- Language selection and file placement under `text/<language>/`.
- Encoding conversion; bytes are raw.
- The language filter (SPEC-002 §3.7) and placeholder text for absent IDs.
