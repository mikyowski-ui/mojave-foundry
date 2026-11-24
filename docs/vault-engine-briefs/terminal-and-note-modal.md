# Owner brief: a data-driven document and terminal reader in `vault-core`

Basis: `vault-engine` at revision `4d5183a`. Modules: `vault_core::ui`
(`pipboy`, `talk`, `flow`; the SPEC-020 §2.1 modal authority),
`vault_core::library` (`LibraryState`, `ManagedWindow`, the SPEC-005 window
words), `vault_core::host_sfall` and `vault_core::host_sfall_rows` (the
`message_box` and `create_win` functions), `vault_vm::extensions` (word sets and
the SPEC-024a mod words), `vault_guidance` (holodisk lists and pages:
`listed_disks`, `RECORDS_PER_PAGE`, `END_DISK`, `END_PARAGRAPH`),
`vault_formats::msg` (`MsgCatalog`), `vault_formats::mod_manifest`,
`vault_platform::map_preview` (drawing and input routing),
`vault_platform::input` (the input log). Specifications: SPEC-002 §3.5
"`.MSG` IDs, Replacement, Errors, and Limits" and §3.6 "`.MSG` Encoding and
Display"; SPEC-005 §2.3 "Complete General-Library Reference:
`0x804C`-`0x80A0`" (the window words, `createwin` `0x8062` to `print`
`0x8072`), §2.5 (`obj_unlock` `0x812F`), §2.7 "Game Dialogue and Barter"
(`start_gdialog` `0x80DE`) and §3.3 "Game Dialogue Session Rules"; SPEC-006 §3.4
"Random Generator and Check Results" and §3.9 "Dialogue Construction and
Choice State Machine"; SPEC-008 §3.8 "Pip-Boy Holodisks and Quest Status";
SPEC-013 §2.2 "Per-Skill Constants" and §3.4 "Default Skill Routine";
SPEC-014 §3.13 "Generic Dialog Box" and §3.41 "Pip-Boy Screen"; SPEC-018
§4.6 "Line tables" and §5.9 "Pip-Boy quests and holodisks"; SPEC-020 §2.1
"Single UI state and hit-test authority"; SPEC-021 §3.10 "Replay
Checkpoints"; SPEC-024a §2.1 "Manifest scripting", §6 "Determinism", §7.1
"The reserved range", §7.2 "Operands", §7.3 "The words", §8 "Out of Scope
in Revision 1"; SPEC-024b §4.11 "Words and functions promoted in Revision
3" (`create_win`, `message_box`).

Tags: `[Must-match]` must be exactly as stated, and changes nothing for a
session without reader rows; `[May-improve]` an implementation freedom that
changes no stated result; `[Safe rule]` what the engine does with data or a
state it cannot honour.

## 1. Purpose

Mods that add readable terminals and documents need a screen that shows
pages of text and a menu of choices, driven by data and opened from a
script, with skill-checked locks and passwords, that replays and saves
deterministically and translates through message files. Today they build
these from dialogue without a head or from script windows, which the engine
does not draw.

What the engine does today (`4d5183a`):

1. **Holodisks.** Fallout 1 has 18 fixed disks, acquired by fixed campaign
   globals, titled `pipboy.msg` 400 + ordinal, with content from
   `1000 × ordinal + 1000` over at most 500 ids, `**END-DISK**`,
   `**END-PAR**` and 35-record pages (SPEC-008 §3.8). Fallout 2 reads them
   from the optional `data/holodisk.txt` (global, title message, first text
   message), acquired when the global is positive (SPEC-018 §4.6, §5.9).
   The reader is the Pip-Boy's Status page (`vault_core::ui::pipboy`).
2. **Dialog box.** The SPEC-014 §3.13 generic box (`lgdialog.frm` or
   `medialog.frm`, Done or Yes/No) is the only engine text box.
3. **`message_box`.** Under sfall mode, `message_box` shows that box as a
   modal screen: the program waits at the next instruction, the world does
   not advance, the result is pushed when it closes, the closing key comes
   from the input log, and it gives -1 where a program cannot wait (a hook
   member, session setup) (SPEC-024b §4.11).
4. **Script windows.** `createwin`, `selectwin`, `display`, `print` and the
   other SPEC-005 window words, and `create_win`, keep their state in
   `LibraryState::windows`; no platform code reads that state, so nothing
   draws. SPEC-024b §4.11 notes that a `create_win` window shows only what
   SPEC-005's words draw. Custom HUD drawing is out of scope (SPEC-024a §8,
   Decision D-6).
5. **Classic computers.** Engine skill use of Science and Repair only shows
   a message; computers work only through object scripts (SPEC-013 §3.4),
   usually as dialogue with no head (`start_gdialog` with head -1, SPEC-005
   §2.7, §3.3).
6. **Free words.** SPEC-024a reserves `0x8300..=0x83FF`, assigns
   `0x8300..=0x8309`, and refuses `0x830A..=0x83FF` at load in every
   session (§7.1). The enhanced range `0x8282..=0x82FF` is also unassigned
   but stays enhanced-preset only (SPEC-017 §2.2).
7. **No font scale.** The only interface size setting is `ui_scale`.

## 2. Proposed API, data and words

### 2.1 Manifest row

```toml
[[readers]]
id = "cell_console"                  # [a-z0-9_-], 1-32 bytes, unique across enabled layers
file = "data/readers/cell_console.toml"
```

A `[[readers]]` row makes its layer scripted (SPEC-024a §2.1), so the words
of section 2.4 load and the `mod_stack` record names the layer.

### 2.2 Document file

```toml
kind = "document"
messages = "tc_docs.msg"   # under text/<language>/game/
title = 900                # message id
first = 1000               # first content id; at most 500 ids are read
style = "paper"            # "paper" or "terminal"
copy_gvar = -1             # a campaign global set to 1 by "take a copy"; -1 none
```

### 2.3 Terminal file

```toml
kind = "terminal"
messages = "tc_term.msg"
header = 100               # first header message id
header_lines = 2           # header records: 100 and 101
footer = 102               # -1 for none
lock_skill = -1            # SPEC-013 skill index, -1 for no skill lock
lock_modifier = 0
attempts = 3
attempts_gvar = -1         # failed attempts are counted here (required with a lock)
unlocked_gvar = -1         # set to 1 on a successful unlock; -1 none
password = -1              # message id of the password, -1 for none
locked_text = 110
lockout_text = 111

[[entry]]
id = "log"
menu = ""                  # "" is the top menu; otherwise an entry id of action "menu"
text = 200                 # the entry's line
condition_gvar = -1        # shown only when the global is at least condition_min
condition_min = 1
action = "page"            # page | menu | set_gvar | unlock | result | close
page = 300                 # page: first content id, read as a document's
gvar = -1                  # set_gvar: the global
value = 0                  # set_gvar: the value
result = 0                 # result: the value pushed when the reader closes

[[entry]]
id = "door"
menu = ""
text = 201
action = "unlock"
result = 2
```

### 2.4 Words

Proposed numbers in SPEC-024a's reserved range; `0x830A..=0x831F` is left
for other pending requests. Pop order is first popped to last popped, as in
SPEC-024a §7.3; operands follow SPEC-024a §7.2.

| Word | Mnemonic | Pop order | Push | Effect |
| --- | --- | --- | --- | --- |
| `0x8320` | `mod_reader_open` | object O, id S | I | Opens the reader; the program waits; pushes the result (section 3.4) |
| `0x8321` | `mod_reader_last` | none | S | The id of the last entry acted on in the most recent reader, or `""` |
| `0x8322` | `mod_reader_exists` | id S | I | 1 when an enabled layer registers `id`, else 0 |

### 2.5 Rust

A new `vault_core::ui::reader` module under the SPEC-020 §2.1 authority:

```rust
pub enum ReaderKind { Document, Terminal }
pub struct ReaderState {
    pub id: String,
    pub kind: ReaderKind,
    pub grid: TextGrid,           // 64 columns × 24 rows of bytes
    pub page: usize,
    pub pages: usize,
    pub menu: String,
    pub selected: usize,
    pub password: Vec<u8>,        // typed so far, at most 16 bytes
    pub phase: ReaderPhase,       // Locked, Password, Menu, Page, LockedOut
}
pub fn open(data: &ReaderData, world: &WorldState) -> Result<ReaderState, ReaderError>;
pub fn input(state: &mut ReaderState, key: ReaderKey, world: &mut WorldState) -> ReaderOutcome;
```

The data files parse in `vault_formats` with the manifest's TOML subset
(strings, integers, `#` comments, array tables).

## 3. Behaviour

### 3.1 Text grid and pages

1. Every reader lays out text on a grid of 64 columns and 24 rows of bytes,
   under every layout and setting, so pagination is a function of the data
   alone. `[Safe rule]`
2. Each message record is one paragraph: it starts a new row and wraps at
   64 columns at the last space; a run with no space is cut at column 64.
   `**END-PAR**` is one blank row. `**END-DISK**` ends the text.
   `**END-PAGE**` starts a new page. `[Safe rule]`
3. Content ids are read in ascending numeric order from `first`, over at
   most 500 ids, as SPEC-008 §3.8 reads a disk; an absent id inside the
   window shows SPEC-002 §3.5's placeholder text and does not stop. `[Safe rule]`
4. A document page: row 0 the title, row 1 blank, rows 2-21 the body, row 22
   blank, row 23 the page indicator `<page>/<pages>` in decimal, right
   aligned to column 63. A body that fills row 21 continues on a new page.
   `[Safe rule]`
5. A terminal menu: rows 0 to `header_lines - 1` the header (wrapped), one
   blank row, then one row per shown entry, `[n] ` and its text, numbered
   from 1 in file order among the entries of the current menu whose
   condition holds; row 23 the footer. At most 9 entries show; more is a
   load error. `[Safe rule]`
6. A terminal `page` action shows its text as a document body with the
   header rows above it, and returns to the menu on Enter or Esc.
   `[Safe rule]`
7. Bytes are drawn as stored; bytes the font has no glyph for draw as its
   space. `[Safe rule]`

### 3.2 Styles and layouts

1. **Classic (640 by 480).** A full-screen modal window. `paper`: the grid
   in font 101 on a plain panel; `terminal`: the grid on black with a
   one-colour font, fixed cell width (the font's widest printable ASCII
   advance plus its letter spacing) and cell height (font height plus 2),
   centred in the frame. `[Safe rule]`
2. **Enhanced, unlocked view.** The same window as a full-screen interface
   frame, framed by the bezel as the Pip-Boy is (`docs/REFERENCE.md`,
   "Bezel"). The grid is identical; it is only drawn larger. `[Safe rule]`
3. **Accessibility.** A `reader_text_scale` setting (1 or 2, default 1)
   draws each glyph at that integer scale where the frame has room, without
   changing the grid or the pagination. `[Safe rule]`
4. Drawing reads `ReaderState` only; no platform copy of the reader's
   state exists (SPEC-020 §2.1 items 3-4). `[Must-match]`

### 3.3 Input

1. Keyboard: Up and Down move the selection; Enter or the entry's digit
   1-9 chooses; PageUp and PageDown (and Left, Right) turn pages; Esc goes
   back one level, and from the top menu or a document closes the reader.
   `[Safe rule]`
2. Mouse: a left click on an entry row chooses it; clicks on the page
   indicator's left or right half turn pages; a right click is Esc.
   `[Safe rule]`
3. Touch: taps arrive as the pointer records they become, as for every
   window (`docs/REFERENCE.md`, touch UI). `[Safe rule]`
4. Password entry: printable bytes `0x20`-`0x7E` append (at most 16),
   Backspace removes one, Enter submits; the typed text shows as `*`. The
   comparison with the password message's text is exact after ASCII case
   folding. A wrong password counts as a failed attempt. `[Safe rule]`
5. Key repeat follows SPEC-014 §3.29's engine repeat. `[Must-match]`
6. While a reader is open, the game's own hotkeys (inventory, Pip-Boy,
   saving, quick save) do nothing. `[Safe rule]`

### 3.4 Opening, locks, actions and results

1. `mod_reader_open` opens a modal screen. The calling program waits at the
   next instruction, and the world does not advance, as for `message_box`
   (SPEC-024b §4.11). `[Safe rule]`
2. It pushes -1 at once, with no screen and one diagnostic, when: the id is
   not registered; it is called in a hook member or in session setup; or
   another modal screen is open. `[Safe rule]`
3. **Lock.** With `lock_skill` set and `unlocked_gvar` not 1, the terminal
   opens on `locked_text` with two choices, use the skill or leave. Using
   it makes one SPEC-006 §3.4 skill check on the player's effective skill
   plus `lock_modifier`: success or critical success unlocks (and sets
   `unlocked_gvar` to 1 when given); failure adds 1 to `attempts_gvar`; a
   critical failure sets `attempts_gvar` to `attempts`. `[Safe rule]`
4. **Password.** With `password` set, an unlocked terminal asks for it
   before the menu; a correct one unlocks as a skill success does.
   `[Safe rule]`
5. **Lockout.** When `attempts_gvar` is at least `attempts`, the terminal
   opens on `lockout_text`, and any key closes it with result -2.
   `[Safe rule]`
6. **Actions.** `page` shows text; `menu` opens the sub-menu whose entries
   name this entry's id in `menu`; `set_gvar` sets the global and stays;
   `unlock` unlocks the object O passed to the open word, as `obj_unlock`
   does (SPEC-005 §2.5), then closes; `result` closes; `close` closes with
   result 0. A null O makes `unlock` a diagnostic and close. `[Safe rule]`
7. **Results.** A terminal pushes the closing entry's `result` (0 for Esc
   and `close`, -2 for lockout). A document pushes 1 when closed on its last
   page, 0 when closed earlier, and 2 when closed by "take a copy".
   `[Safe rule]`
8. **Take a copy.** A document with `copy_gvar` shows a `C` choice on its
   last page that sets the global to 1 and closes. Under Fallout 2 a
   `data/holodisk.txt` record with that global then lists it in the
   Pip-Boy (SPEC-018 §5.9). `[Safe rule]`
9. **Experience.** The engine grants no experience for a skill unlock; a
   script may, on the pushed result. `[Safe rule]`

## 4. Validation and errors

Each of these stops the launch, with `mod_list:` and the layer's name, as a
malformed manifest does today. `[Safe rule]`

1. A `[[readers]]` row with a bad `id`, a duplicate `id` across enabled
   layers, or a `file` the layer does not hold.
2. A reader file with an unknown key, a missing required key, or a `kind`
   other than `document` or `terminal`.
3. A message file name that is not a single path component ending `.msg`.
4. A terminal with a lock or a password but no `attempts_gvar`; `attempts`
   below 1 or above 99; a `lock_skill` outside SPEC-013's skill indexes.
5. An entry with a duplicate `id`, a `menu` that names no entry of action
   `menu`, a menu cycle, an unknown `action`, or more than 9 entries in one
   menu.
6. A global index below -1, or outside the campaign's variable count when
   the session starts (checked then, with the same message).

At run time:

7. A missing message file shows the placeholder text for every id, with
   one diagnostic per reader. `[Safe rule]`
8. A word's failed operand follows SPEC-024a §7.2: no effect, a diagnostic,
   and the default push (-1, `""` or 0). `[Safe rule]`

## 5. Determinism property

1. Every choice comes from logged key and pointer records, so a replay gives
   the same screens, results, global writes and draws; no new input record
   is needed, as for `message_box`. `[Must-match]`
2. The only world effects are the global writes, the object unlock, the
   skill check's random draws (in the shared generator, at the choice step)
   and the pushed result. Each enters `state_hash` only through covered
   state it changes (SPEC-021 §3.1). `[Must-match]`
3. Reader state is UI state: not saved, outside every hash. Attempts and
   unlocks persist only through the globals the data names, so `SAVE.DAT`
   does not change (SPEC-024a §6 item 10). `[Must-match]`
4. Pagination never depends on the window, `ui_scale` or
   `reader_text_scale`. `[Must-match]`
5. Text comes from `.msg` files under `text/<language>/game/`, so a
   translation is a message file with the same ids. `[Safe rule]`
6. A session with no `[[readers]]` row has no new word, file or screen.
   `[Must-match]`

## 6. Edge cases

- Every entry of a menu hidden by its condition: the menu shows only the
  header and footer; Esc still works.
- A password message with trailing spaces: compared as stored after case
  folding, so authors keep passwords trimmed.
- `attempts_gvar` already above `attempts` from a save: lockout.
- A reader opened from `map_enter_p_proc` during a map load: the modal opens
  after the load completes, as other modal screens do.
- A document with no content before `**END-DISK**`: one page with the title
  only.
- A 64-byte word: fills one row exactly; a 65-byte one is cut.
- Combat: the word may open a reader in combat; the combat turn does not
  advance while it is open.

## 7. Synthetic test vectors

Vector A, a two-entry terminal. `tc_term.msg`:

```text
{100}{}{SECURITY CONSOLE 7}
{101}{}{WELCOME, OPERATOR}
{102}{}{ESC: LOG OFF}
{200}{}{Read duty log}
{201}{}{Open cell door}
{300}{}{Day 1. Quiet.}
{301}{}{**END-DISK**}
```

The terminal file of section 2.3 (no lock, no password; entry `log` with
`page = 300`, entry `door` with `unlock`, result 2). Expected grid on open
(rows not listed are 64 spaces):

```text
row  0: SECURITY CONSOLE 7
row  1: WELCOME, OPERATOR
row  2:
row  3: [1] Read duty log
row  4: [2] Open cell door
row 23: ESC: LOG OFF
```

Key `Digit1` gives rows 0-1 the header, row 2 blank, row 3 `Day 1. Quiet.`;
`Escape` returns to the menu; `Digit2` unlocks O, closes, and pushes 2;
`mod_reader_last` then pushes `door`.

Vector B, a two-page document. `tc_docs.msg`:

```text
{900}{}{DUTY LOG}
{1000}{}{Day 1. Quiet.}
{1001}{}{**END-PAGE**}
{1002}{}{Day 2. The door jammed.}
{1003}{}{**END-DISK**}
```

Page 1: row 0 `DUTY LOG`, row 2 `Day 1. Quiet.`, row 23 `1/2` in columns
61-63. `PageDown` gives page 2: row 2 `Day 2. The door jammed.`, row 23
`2/2`. `Escape` there pushes 1; `Escape` on page 1 pushes 0.

Vector C, a lock. As A with `lock_skill = 12`, `lock_modifier = 0`,
`attempts = 3`, `attempts_gvar = 640`, global 640 at 0, game tick below
864000 (no critical upgrade draw), and the player's effective Science 40.
With a generator whose next 1-100 draw is 41 the check fails (margin -1),
global 640 becomes 1, and the locked screen stays; with a draw of 40 it
succeeds and the menu of vector A shows. With global 640 at 3 on open, the
lockout text shows and any key pushes -2.

Vector D (refused at launch): `id = "Cell Console"`; an entry whose `menu`
is `"nowhere"`; a terminal with `password = 120` and `attempts_gvar = -1`;
ten entries in one menu.

## 8. Open questions for the owner

1. **Word numbers.** `0x8320`-`0x8322`, leaving `0x830A`-`0x831F` free?
   Recommended: yes; the owner assigns all pending mod words in one
   SPEC-024a revision.
2. **Scripted layers.** Should a `[[readers]]` row make its layer scripted
   (and so write the `mod_stack` record)? Recommended: yes, so replays
   compare the layers that define the readers.
3. **Procedure actions.** Should an entry be able to run a procedure of the
   opening script as a host-initiated call before the program resumes?
   Recommended: not in this revision; the `result` value lets the script
   call its own procedure, with no re-entry rules to add.
4. **Object binding.** Should a reader open from an object's use with no
   script, by PID? Recommended: no; scripts open readers from
   `use_p_proc`, as classic computers do.
5. **Fallout 1 copies.** Fallout 1 holodisks are a fixed table. Should a
   layer's `data/holodisk.txt` also apply under `fallout1` so "take a copy"
   lists there? Recommended: yes, as an owner amendment of SPEC-008 §3.8
   limited to layers; until then `copy_gvar` under `fallout1` sets the
   global only, with a diagnostic at launch.
6. **Grid size.** Is 64 by 24 the right fixed grid? Recommended: yes; it
   fits 640 by 480 with any shipped font at the cell rule of section 3.2.
7. **Text scale values.** Only 1 and 2? Recommended: yes, integer scales,
   no resampling.
8. **Sound.** Should readers play a key click and a page sound?
   Recommended: the SPEC-014 button sound `ib1p1xx1` on each choice, nothing
   else.
9. **Draw the script windows.** Should the engine also start drawing
   SPEC-005 script windows? Recommended: no, separate request; this reader
   does not depend on them.

## 9. Out of scope

- Free-form drawing, images in readers, and custom HUD drawing (SPEC-024a
  §8).
- Mini-games beyond the skill check and password.
- Changing dialogue, the generic dialog box or `message_box`.
- Editing reader data from scripts at run time.
