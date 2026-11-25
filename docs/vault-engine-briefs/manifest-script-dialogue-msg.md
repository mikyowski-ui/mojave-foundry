# Owner brief: dialogue message files for script rows a manifest registers

Basis: `vault-engine` at revision `4d5183a`. Modules: `vault_core::assets`
(the `WorldAssets` trait and its loose-file implementation: `program`,
`program_by_name`, `local_count`, `dialogue_path`, `message`,
`dialogue_catalog`, `with_registry`, `registry`), `vault_core::host_ai`
(`script_message`, the per-script catalog cache), `vault_core::host_talk`
(`spoken_message`, the reply and review-history code),
`vault_core::host_game` (the `message_str` word),
`vault_formats::mod_manifest` (`ScriptRegistration`, `ModRegistry::build`,
`ModRegistry::from_vfs`), `vault_formats::vfs` (`Vfs::layer_manifests`,
the layered mount) and `vault_formats::msg` (`MsgCatalog`). Specifications:
SPEC-002 §1 (language folders), §3.5 "`.MSG` IDs, Replacement, Errors, and
Limits" and §3.7 "Language Filter"; SPEC-005 §2.7 "Game Dialogue and
Barter" and §3.3 "Game Dialogue Session Rules" (the message-list number);
SPEC-006 §3.9 "Dialogue Construction and Choice State Machine" ("Speech
requests", "Review history", "Ask About lookup"); SPEC-023 §3.4 "Mod Layers
in Memory"; SPEC-024a §2.1 "Manifest scripting".

Tags: `[Must-match]` must be exactly as stated, and changes nothing for a
session with no manifest script rows; `[May-improve]` an implementation
freedom that changes no stated result; `[Safe rule]` what the engine does
with data it cannot honour.

## 1. Purpose

A mod layer that adds dialogue scripts registers their rows in its
`vault-mod.toml` with `[[scripts]]`, so that it does not have to replace
`scripts/scripts.lst`. Such a script shows its lines from the per-script
dialogue message file `text/<language>/dialog/<name>.msg`. Today that file
is not found for a manifest row, so the layer must ship a whole replacement
`scripts.lst`, and two layers cannot both do so. This brief asks that the
dialogue lookup use the same registry that already finds the script's
program, so that a manifest row finds its dialogue file exactly as a
`scripts.lst` row does.

## 2. What happens today (`4d5183a`)

1. **The row is registered.** A `[[scripts]]` table has `file` (a name
   ending in `.int`, stored lowercase), `index` (0 to `0x00FFFFFF`) and an
   optional `local_vars` (`vault_formats::mod_manifest::ScriptRegistration`).
   `ModRegistry::from_vfs` reads every enabled layer's manifest, lowest
   layer first (`Vfs::layer_manifests`), and `ModRegistry::build` keeps one
   map from index to registration. Two layers registering one index is an
   error naming both (`script index N is registered by both ...`).
   (SPEC-023 §3.4: a memory layer behaves as a folder layer.)
2. **The row is refused if `scripts.lst` has it.**
   `vault_core::assets` `with_registry` (run when the assets are built)
   fails with `mod registration: script index N is a scripts.lst row` when
   any registered index is also a row of the base list. So a row is in
   `scripts.lst` or in a manifest, never both, and no precedence is ever
   exercised at run time.
3. **The program is found through the registry.**
   `WorldAssets::program(index)` takes the name from the `scripts.lst` row
   first and falls back to the registered row (`.or(registered)`), and
   loads `scripts/<name>`. `local_count(index)` does the same for local
   variables. A manifest row therefore runs.
4. **The dialogue file is not.** The dialogue words take a message-list
   number L (SPEC-005 §3.3): L is the one-based list, so list L is the
   message file of zero-based row L - 1 (SPEC-006 §3.9, "Ask About
   lookup"). `message_str`, `gsay_reply`, `gsay_option`, `gsay_message`,
   `giq_option` and the Ask About lookup all reach
   `vault_core::host_ai::script_message(L, id)`, which asks
   `WorldAssets::dialogue_catalog(L)` and `WorldAssets::message(L, id)`.
   Both call `dialogue_path(L)`. That function reads `scripts/scripts.lst`
   alone: it takes row L - 1, strips `.int` from the row's file name, and
   gives `text/<language>/dialog/<stem>.msg`. It does not read
   `registry().scripts`, so for a manifest row it returns `None`, whatever
   files exist. `None` is read as a missing message.
5. **A missing message is `Error`, never a panic.** `message_str` pushes the
   text `Error` (SPEC-005 §3.3). A reply or option reads as `Error`
   (SPEC-006 §3.9). A missing or unreadable file writes no diagnostic line
   today (only a file that stops early does, SPEC-002 §3.5); the failed
   lookup is cached, bounded.
6. **Layering of the file itself already works.** The message file is read
   through the layered `Vfs`, so a higher layer's `dialog/<name>.msg`
   shadows a lower layer's, like any other file. Only the step from the
   row to `<name>` is missing for manifest rows.

The gap is real: a manifest-registered row finds its program but not its
dialogue file.

## 3. Synthetic reproduction

All of it synthetic. Layer `alpha` carries:

```toml
name = "Alpha"
[[scripts]]
file = "synth_a.int"
index = 4096
```

with `scripts/synth_a.int`, a program with one procedure, `talk_p_proc`,
that calls `start_gdialog` with head -1, `gsay_start`, `gsay_reply` with
message list 4097 and message number 100, then `gsay_end`; and
`text/english/dialog/synth_a.msg`:

```text
{100}{}{Line one.}
{101}{}{Line two.}
```

The base `scripts.lst` has fewer than 4,096 rows. Talking to an object that
carries the row (row 4096, list 4097) shows the reply `Error` today. The
expected text is `Line one.`

## 4. Requested behaviour

1. **One resolver.** The row-to-name step of `program`, `local_count` and
   `dialogue_path` becomes one function (for example
   `WorldAssets::script_file_name(index)`), which reads the `scripts.lst`
   row first and the registered row second, as `program` does now.
   `dialogue_path(L)` uses it with `index = L - 1`: the stem is the file
   name without `.int`, and the path is
   `text/<language>/dialog/<stem>.msg`. `[Must-match]`
2. **Same words, same answers.** `message_str`, the reply and option words,
   `gsay_message`, the review history and the Ask About lookup then give a
   manifest row the text a `scripts.lst` row with that file name would
   give. `[Must-match]`
3. **Every layer order.** The file is found through the layered `Vfs`, so
   with several layers the row comes from the layer that registered it and
   the file from the highest layer that has the path, for any order of
   enabled layers. Registering a row in a manifest needs no
   `scripts.lst` in the layer. `[Must-match]`
4. **No change for base rows.** A `scripts.lst` row resolves exactly as
   today, and a session with no `[[scripts]]` rows takes the same path and
   gives the same bytes in saves and replays. `[Must-match]`
5. **Missing file.** A registered row whose `dialog/<name>.msg` is absent,
   unreadable or truncated behaves as a `scripts.lst` row does: the text
   reads `Error`, the program goes on, and the engine never panics.
   `[Safe rule]` One diagnostic line naming the path, written once per
   path, is `[May-improve]`.
6. **Row in both.** Keep today's rule: a registration whose index is a
   `scripts.lst` row fails at setup with the message above, so neither
   source wins. If both ever coexist, `scripts.lst` is read first in all
   three lookups. `[Must-match]`
7. **List 0 and below.** Unchanged: no file, text `Error` (SPEC-005 §3.3).
   `[Must-match]`

## 5. Synthetic test vectors

Rows use index 4096 and up; the list number is index + 1. Dialogue files
are under `text/english/dialog/`.

| Id | Setup | Action | Expected |
| --- | --- | --- | --- |
| D1 | Layer `alpha` as in section 3, with `synth_a.msg` | `talk_p_proc` replies with list 4097, id 100 | Reply text `Line one.`; review history entry (4097, 100) |
| D2 | As D1 | `message_str(4097, 101)` | `Line two.` |
| D3 | As D1 | `message_str(4097, 102)` | `Error` (absent id, as for a base row) |
| D4 | Layers `alpha` (row 4096, `synth_a.msg`) and `beta` (row 4097, `synth_b.msg` with `{100}{}{Other one.}` and `{101}{}{Other two.}`), order `alpha` then `beta` | Reply with list 4097 id 100; reply with list 4098 id 100 | `Line one.`; `Other one.` |
| D5 | As D4 with the order `beta` then `alpha` | The same two replies | The same two texts |
| D6 | As D4, then `beta` also ships `dialog/synth_a.msg` with `{100}{}{Shadowed.}` | Reply with list 4097 id 100 | `Shadowed.` when `beta` is the higher layer; `Line one.` when `alpha` is |
| D7 | Layer `alpha` with no `dialog/synth_a.msg` | Reply with list 4097 id 100; `message_str(4097, 100)` | `Error` both times; no panic; the script runs on |
| D8 | Layer `gamma` registers row 4096 and `synth_c.int`, with a truncated `synth_c.msg` that holds `{100}{}{Kept.}` and then an unclosed field | `message_str(4097, 100)` | `Kept.` (SPEC-002 §3.5: records before the failure load) |
| D9 | Layers `alpha` and `delta`, both registering index 4096 | Build the assets | Setup error naming both layers |
| D10 | Layer `alpha` registers index 0 | Build the assets | Setup error `script index 0 is a scripts.lst row`; unchanged from today |
| D11 | No manifest rows; a base `scripts.lst` row 3 naming `synth_base.int`, with `dialog/synth_base.msg` | `message_str(4, 100)` | The same text as before the change; the same replay and save bytes |
| D12 | Layer `alpha` as D1 | Ask About lookup on an object whose script is row 4096, with `{1000}{}{ping}` and `{1100}{}{pong}` in `synth_a.msg` | Token `ping` answers `pong` (SPEC-006 §3.9) |
| D13 | Layer `alpha` (row 4096) and the file `synth_a.msg` in `text/french/dialog/` only, language `english` | `message_str(4097, 100)` | `Error` (no second language is tried, SPEC-002 §1) |

## 6. Out of scope

- A `dialog` key in `[[scripts]]` or any other way to name a message file
  that differs from the program's name.
- Message lists for other message files (`game/`, `proto`), which do not go
  through the script list.
- Changing which rows may be registered, the index range, or the rule that
  one index has one owner.
- Hot reload of message files already cached for a list.

## 7. Open questions

- **Q1.** With no loadable `scripts/scripts.lst`, `program` fails today
  (`?` on the list read) and `dialogue_path` gives `None`. Default: keep
  both as they are; a layer-only install is not a request here.
- **Q2.** The stem rule strips `.int` from the stored lowercase name, as it
  does for a `scripts.lst` row. A row's file name with other letters
  (`SYNTH_A.INT` in the manifest) is stored lowercase by the parser.
  Default: the dialogue file is `synth_a.msg`, looked up as any `Vfs`
  path.
