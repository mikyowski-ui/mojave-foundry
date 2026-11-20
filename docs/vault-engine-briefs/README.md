# Owner briefs for `vault-engine`

Requests for production writers in `vault-engine`, written in generic
Fallout 1 and 2 engine terms. Each one is derived only from `vault-engine`'s
own source and approved specifications at the pinned revision (`3adeac9`),
cites the module and specification sections it rests on, and tags each
requirement `[Must-match]`, `[May-improve]` or `[Safe rule]`.

| Brief | Asks for |
| --- | --- |
| [frm-writer.md](frm-writer.md) | An FRM writer in `vault-formats` that `FrmArchive::parse` reads back frame for frame, with adjacent sequence sharing |
| [lst-writer.md](lst-writer.md) | An LST writer and appending editor in `vault-formats`, one position per line for every list kind, with the 4,096-entry art index limit |
| [msg-writer.md](msg-writer.md) | A MSG writer in `vault-formats` that refuses text the brace format cannot carry, and appends to an existing file |
| [gam-writer.md](gam-writer.md) | A GAM writer in `vault-formats` for map and campaign variable initializers |
| [int-assembler.md](int-assembler.md) | A small `.INT` assembler and source listing in `vault-vm`, round-tripped through `IntProgram::parse_in`, with open questions |

Until a writer lands upstream, this repository's own adapter in
`crates/emit` (behind the `FormatWriters` trait) produces the same bytes;
the synthetic FRM, LST, MSG and GAM vectors in the briefs are checked
against it (`crates/emit/tests/brief_vectors.rs`).
