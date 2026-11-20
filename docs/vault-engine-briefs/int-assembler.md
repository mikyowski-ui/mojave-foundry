# Owner brief: an INT assembler in `vault-vm`

Basis: `vault-engine` at revision `3adeac9`, the `vault-vm` modules
`program` (`IntProgram::parse_in`, `WordSet`, `Procedure`), `machine` (how
instructions consume operands), `stack` (`Value` tags) and `extensions`
(`opcode_in`, the host word names), and SPEC-004 §2.1 "Numeric Conventions
and Overall File Layout", §2.2 "Bootstrap and Return-Trampoline Area",
§2.3 "Procedure Records and Flags", §2.4 "Identifier and Literal String
Records", §2.5 "Instruction Words, Immediates, and Tagged Stack Records",
§3.2-§3.6 (the core words and the call protocol) and §4.1 "Complete
`.INT`: Two Procedures, Strings, a Call, and a Branch"; SPEC-017 §2.1 and
§8 for the word sets and the stack model.

Tags: `[Must-match]` the bytes must be exactly as stated; `[May-improve]`
the loader's existing safe rule; `[Safe rule]` what the assembler does
with input it cannot represent.

## 1. Purpose

A mod-authoring tool needs to produce small compiled scripts: map scripts
that set variables on entry, object scripts that answer `description_p_proc`
or `use_p_proc`, and test programs. Today the only way to get a `.INT` is a
compiler outside the engine. This brief asks for a small assembler in
`vault-vm`: a line-oriented text syntax that maps one to one onto the
program layout the loader reads, plus a source listing for the reverse
direction, both checked against `IntProgram::parse_in`. It is not a
compiler for the scripting language.

## 2. Proposed API

A new public module `asm` in `vault-vm`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct AsmError { pub line: usize, pub message: String }

/// Assemble `source` for `words` (and the enhanced range when `enhanced`):
/// a word the set does not accept is an error, as it is for the loader.
pub fn assemble(source: &str, words: &WordSet, enhanced: bool) -> Result<Vec<u8>, AsmError>;

/// Source text for a parsed program, which `assemble` turns back into the
/// same bytes when the program's tables are in the canonical form of §3.
pub fn to_source(program: &IntProgram) -> String;
```

## 3. Program layout, as the loader reads it

All numbers big-endian. `[Must-match]` for the whole section.

| Offset | Bytes | Content |
| --- | ---: | --- |
| `0x00` | 42 | Bootstrap and return trampolines (SPEC-004 §2.2), decoded as instructions by the loader |
| `0x2A` | 4 | Procedure count N, at most 65536 |
| `0x2E` | 24 × N | Procedure records |
| I = 46 + 24N | 4 | Identifier payload length A (not negative) |
| I + 4 | A | Identifier records |
| I + 4 + A | 4 | `FF FF FF FF` |
| T | 4 | Literal payload length B, or `FF FF FF FF` for no table |
| T + 4 | B | Literal records (when present), then `FF FF FF FF` |
| after the tables | ... | Code, decoded as instructions to the end of the file |

- A procedure record is name offset, flags, deadline, condition address,
  body address, argument count (SPEC-004 §2.3). The loader requires the
  name offset to be the start of an identifier text, the argument count to
  be 0 to 341, and the body address (and a nonzero condition address) to be
  an instruction start or the file length.
- A string record is a 16-bit length S, then S bytes holding the text, a
  NUL and padding. A reference addresses the first text byte, relative to I
  for identifiers and to T for literals, so the first record's text is at
  offset 6 (SPEC-004 §2.4). The loader requires S ≥ 1, the record inside
  the payload, and a NUL within it.
- An instruction is a 16-bit word; a word whose low ten bits are 1 is a
  push with a 32-bit payload (six bytes), every other word is two bytes
  (SPEC-004 §2.5). The loader accepts the 32 push words with bit 15 set
  and the non-push words of the selected `WordSet`.
- Addresses (bodies, conditions, jump targets) are absolute file offsets.

The assembler's canonical form: one identifier record per distinct name,
in first-declaration order, with `S = 2 × ceil((K + 1) / 2)` for K text
bytes and zero padding; literals the same, in declaration order; no
literal table (`FF FF FF FF`) when there are no literals; code right after
the tables. `[Must-match]` (it reproduces SPEC-004 §4.1 exactly)

## 4. Proposed assembly syntax

Line-oriented; `;` starts a comment; blank lines ignored; names are
case-sensitive in the source and written as given.

```text
.procedure <name> [args=<n>] [flags=<n>] [condition=<label>]
                               ; next procedure record, in order; its body is label <name>
.name <name>                   ; an identifier record with no procedure (external variables)
.literal <label> "<text>"      ; a literal record; \" \\ and \xHH escapes in the source only
.entry <label>                 ; where the bootstrap's jump goes (SPEC-004 §2.2, offset 0x0A)
<label>:                       ; an address
    push.i <i32>               ; C001
    push.f <decimal>           ; A001, binary32 nearest
    push.s <literal label>     ; 9001, literal offset
    push.n <name>              ; identifier offset (tag: open question Q1)
    push.a <label>             ; C001, absolute address
    push.p <procedure name>    ; C001, procedure index
    push.raw <tag> <u32>       ; any of the 32 push words, payload as given
    <mnemonic>                 ; a core word by the table below, or a host word by its
                               ; `extensions::opcode_in` name for the selected word set
    op <0xNNNN>                ; any accepted non-push word
```

The assembler always emits the SPEC-004 §2.2 bootstrap with `.entry`'s
address in the push at offset `0x0A`. `[Must-match]` A source with no
`.entry` is an error. `[Safe rule]`

Core mnemonics (SPEC-004 §3.2-§3.5): `nop` 8000, `critical_on` 8002,
`critical_off` 8003, `jmp` 8004, `call` 8005, `schedule_at` 8006,
`schedule_when` 8007, `r_to_v` 800C, `v_to_r` 800D, `end` 8010,
`stop` 8011, `fetch_global` 8012, `store_global` 8013, `fetch_external`
8014, `store_external` 8015, `export_var` 8016, `export_proc` 8017, `swap`
8018, `swap_r` 8019, `pop` 801A, `dup` 801B, `ret` 801C, `ret_yield` 801D,
`pop_r` 801E, `restore_base` 8029, `drop_frame` 802A, `frame` 802B,
`global_area` 802C, `proc_addr` 802D, `bz` 802F, `loop_bnz` 8030,
`store_local` 8031, `fetch_local` 8032, `eq` 8033, `ne` 8034, `le` 8035,
`ge` 8036, `lt` 8037, `gt` 8038, `add` 8039, `sub` 803A, `mul` 803B, `div`
803C, `mod` 803D, `and` 803E, `or` 803F, `band` 8040, `bor` 8041, `bxor`
8042, `bnot` 8043, `ftoi` 8044, `not` 8045, `neg` 8046, `wait` 8047,
`cancel` 8048, `cancel_all` 8049. The remaining core words go through
`op`. These names are proposals; the owner may rename them.

## 5. Validation and errors

`assemble` returns `AsmError` with the source line, and no bytes, for:

- an unknown directive, mnemonic or label, or a label defined twice;
  `[Safe rule]`
- a word the chosen `WordSet` (with `enhanced`) does not accept, exactly
  as `WordSet::accepts_in` decides; `[Must-match]`
- more than 65536 procedures, an argument count over 341, a string over
  65533 text bytes, or a name or literal containing NUL; `[May-improve]`
  (the loader's own limits)
- a condition label that resolves to address 0 (the loader reads 0 as no
  condition); `[Safe rule]`
- an image over the loader's 64 MiB limit, or a value outside its field.
  `[Safe rule]`

## 6. Round-trip property

For every source the assembler accepts, `p = IntProgram::parse_in(&assemble(s, w, e)?, w, e)?`
succeeds; `p.procedures()` lists the declared procedures in order with
their names, flags, deadline 0, condition and body addresses, and argument
counts; `p.identifier(offset)` and `p.literal(offset)` give each declared
string at the offset `push.n` / `push.s` used; and `p.boundary(a)` is true
for the address of every assembled instruction. For every program whose
tables are canonical, `assemble(&to_source(&p), w, e)? == p.image()`.
`[Must-match]`

## 7. Synthetic test vectors

Vector A (SPEC-004 §4.1). This source must assemble to the fixture's 282
bytes exactly, and running it must give the fixture's final state:

```text
.procedure main
.procedure sumxy args=2
.literal greeting "hi"
.entry init
init:
    global_area
    push.i 0
    push.i 0
    critical_off
    push.a main
    jmp
main:
    frame
    push.a after
    v_to_r
    push.i 5
    push.i 7
    push.i 2
    push.p sumxy
    call
after:
    push.a else
    swap
    push.i 10
    gt
    bz
    push.i 99
    push.i 0
    store_global
    push.a join
    jmp
else:
    push.i -1
    push.i 0
    store_global
join:
    drop_frame
    restore_base
    ret
sumxy:
    frame
    push.i 0
    fetch_local
    push.i 1
    fetch_local
    add
    v_to_r
    drop_frame
    r_to_v
    restore_base
    ret
```

Vector B: a program with no literals. `.procedure start`, `.entry init`,
`init:` `push.i 0` `critical_off` `push.a start` `jmp`, `start:` `frame`
`drop_frame` `restore_base` `ret`. N is 1, I is `0x46`, the identifier
payload is 8 bytes (`00 06` `start` `00`), the literal table word
`FF FF FF FF` is at T = `0x56` and the code starts at T + 4 = `0x5A`, where
the bootstrap's jump goes. `IntProgram::parse_in` accepts it under
`WordSet::FALLOUT2`, and running it reaches the end instruction at offset
18 with both stacks empty.

Vector C (refused): a host word outside `WordSet::FALLOUT1` assembled for
that set; `push.a nowhere` with no such label; `.procedure x args=342`.

## 8. Open questions for the owner

The loader and VM source at this revision do not pin these down:

1. **Identifier operand tag.** `fetch_external`, `store_external` and
   `export_var` pop a value and use only its payload as an identifier
   offset; the loader and VM never look at its tag. Which tag should
   `push.n` emit (integer `C001`, static string `9001`, or the generic
   `8001`)? The brief proposes `C001` until told otherwise.
2. **Name record sharing.** May two procedure records share one identifier
   record, and should procedure and external-variable names share records?
   The loader allows both; the canonical form above shares equal names.
3. **Initial stream.** SPEC-004 §2.2 describes the shape every observed
   initial stream has (globals, exports, root argument count, jump to
   `start`). Should the assembler generate it from directives (`.global`,
   `.export`) instead of leaving it to the source?
4. **Index 0.** Must record 0 be the root procedure sharing `start`'s
   body, or any procedure? The lifecycle fallback uses index 1 when a name
   is missing (SPEC-004 §3.7), so the choice is visible.
5. **Body at end of file.** The loader accepts a body address equal to the
   file length. Is an empty trailing procedure meaningful, or should the
   assembler refuse it?
6. **Export name length.** Registries keep 31 bytes of a name
   (SPEC-004 §3.2). Should the assembler refuse longer exported names?
7. **Zero-length literal table.** The loader accepts a present table of
   length 0 as well as the absent form. Should `to_source` keep that
   distinction (it changes bytes)?
8. **Procedure flags and deadlines.** Fresh files hold flags 0 and
   deadline 0 everywhere observed. Should the assembler allow nonzero
   values at all?
9. **Host word mnemonics.** Should the host names be the `extensions`
   table's names for the selected word set only, and how should a word
   whose name differs between word sets be written?
10. **Verification.** Should `assemble` also run the SPEC-017 §8 stack
    model (`stack_check`) and refuse a finding, as the engine does for
    scripts a mod layer serves?

## 9. Out of scope

- A compiler for the scripting language, macros, or optimisation.
- New instruction words or host words.
- Saved-game script state.
