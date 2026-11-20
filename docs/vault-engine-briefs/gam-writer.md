# Owner brief: a GAM writer in `vault-formats`

Basis: `vault-engine` at revision `3adeac9`, the `vault-formats` module
`gam` (`GamFile::parse`, `GamSection`) and SPEC-006 §2.1 "`.GAM`: Ordered
Text Initializers" and §4.1 "`.GAM`: Index Assignment and Parser
Boundaries".

Tags: `[Must-match]` the bytes must be exactly as stated; `[May-improve]`
the parser's existing safe rule; `[Safe rule]` what the writer does with
input it cannot represent.

## 1. Purpose

A mod-authoring tool needs to write map variable files
(`maps/<map>.gam`, `MAP_GLOBAL_VARS:`) for new maps, and campaign variable
files (`GAME_GLOBAL_VARS:`). Only the order of the values reaches the
engine: a variable's index is its position after the marker. A writer must
produce exactly one index per variable, with its value, and must not let a
name be read as syntax. This brief asks for that writer.

## 2. Proposed API

In the `gam` module:

```rust
impl GamFile {
    /// The marker line, then one `VAR_<index> = <value>;` line per value.
    pub fn write(&self) -> Result<Vec<u8>, ParseError>;
    /// As `write`, with documentary names (one per value).
    pub fn write_named(&self, names: &[&str]) -> Result<Vec<u8>, ParseError>;
}
```

Errors use `ParseError` with format "GAM" and the zero-based variable index
as offset. A file written for one section is parsed with that section.

## 3. Layout, as the parser reads it

`[Must-match]` for the whole section.

- The input is read in chunks of at most 257 bytes, each ending after an LF
  if one comes first; CRLF reads as LF (SPEC-006 §2.1).
- Chunks are skipped until one starts with the section's 16-byte marker:
  `GAME_GLOBAL_VARS` (its conventional colon is not compared) or
  `MAP_GLOBAL_VARS:` (colon included). The marker chunk allocates nothing.
- After it, every chunk allocates one value, in order, except a chunk that
  starts with LF and a chunk that starts with `//`. There is no end of
  section.
- The text of a chunk ends at its first `;` or NUL. The value is the
  decimal prefix after the first `=` in that text (leading whitespace and a
  sign allowed); no `=` gives 0; no digits give 0 and an overflowing value
  is clamped (`[May-improve]` in SPEC-006 §2.1).

The writer emits the marker followed by LF, then for each value
`<name> = <value>;` and LF, with the value in plain decimal.
`[Must-match]`

## 4. Validation and errors

Refused, writing nothing:

- a name that is empty, or holds a byte outside printable ASCII and space
  (an LF would end the chunk; a CR or NUL would change it); `[Safe rule]`
- a name containing `;` or `=`, or starting with `//`; `[Safe rule]`
- a name starting with either section marker; `[Safe rule]`
- a line longer than 257 bytes with its LF (it would allocate two
  indexes); `[Must-match]`
- more names than values, or fewer, in `write_named`; `[Safe rule]`
- more than the parser's one million values. `[Safe rule]`

Duplicate names are allowed: each line is its own index. `[Must-match]`

## 5. Round-trip property

For every accepted file `g`, `GamFile::parse(&g.write()?, g.section)?` has
`found == true` and `values == g.values`; the same holds for
`write_named`. A file written for one section parsed for the other has
`found == false`. `[Must-match]`

## 6. Edge cases

- No values: the marker line alone, which parses as found and empty.
- Values `i32::MIN`, `0`, `i32::MAX`.
- A 257-byte line with its LF (accepted) and a 258-byte one (refused).
- Names with spaces and with a single leading `/`.

## 7. Synthetic test vectors

- Map section, values `[0, -17, 2147483647]`, `write()` gives
  `MAP_GLOBAL_VARS:\nVAR_0000 = 0;\nVAR_0001 = -17;\nVAR_0002 = 2147483647;\n`
  (four-digit zero-padded indexes, widened when there are more than
  10,000), which parses to `[0, -17, 2147483647]`, found.
- Campaign section, `write_named(&["A"])` for `[5]` gives
  `GAME_GLOBAL_VARS:\nA = 5;\n`.
- The map vector above parsed as the campaign section: found false,
  values empty.
- Refused: names `a;b`, `a=b`, `//c`, `MAP_GLOBAL_VARS:x`, and a name of
  250 bytes with value `-2147483648`.

## 8. Out of scope

- Which variables a campaign or map needs; the file carries only values.
- Saved-game variable storage (a different layout).
