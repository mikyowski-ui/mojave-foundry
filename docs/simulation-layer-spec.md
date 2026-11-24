# Simulation layer: design for Pass 2

Status: design only, 2026-11-23. Nothing here is built. `crates/transpile`
is still a placeholder; this document is its blueprint, and section 12 is
the order to build it in. When the build starts, the lead copies the
interfaces it settles into `docs/contracts.md` (a new section 11) first, as
AGENTS.md requires.

Basis:

- **Engine side**, read only from `vault-engine`'s committed head
  (`4d5183a`) with `git show`, never from its working tree: the approved
  specs SPEC-004, 005, 006, 007, 009, 013, 014, 017, 018, 019, 021, 024a,
  024b and `docs/REFERENCE.md`, and the modules `vault_core::assets`,
  `vault_core::host_game`, `vault_vm::extensions`,
  `vault_formats::{map, mod_manifest, lst}`. Our `vault-formats` pin is
  still `3adeac9`; the manifest tables used below (`[[scripts]]`,
  `[[global_scripts]]`, `[[hooks]]`) exist there and at `df2bf4c`. Where a
  fact below comes from code, not a spec, the module is named.
- **Source side**, from the typed records in `fnv_esm::records`, the MIR
  schema in `mir::db`, and a read-only measurement of the owner's
  `Fallout3.esm` (GOTY) by the probe and by a `python3 -I` pass that wrote
  only scratch files (appendix B). Record semantics not visible in the data
  come from how the editor presents them; each such reading is marked
  **calibration** where it needs checking against the original game.
- No game text appears below: editor ids, counts and structure only.

## 0. Summary

Pass 2 turns the dialogue, quests, scripts, AI packages, enable-parent
groups, terminals and notes of a small Fallout 3 slice into Fallout 2
`.int` programs, `.msg` files and data tables inside the existing mod
layer. Everything is lowered onto facilities the engine already has:
dialogue is the SPEC-006 §3.9 reply/option machine, quest state lives in
campaign globals, the Pip-Boy reads `quests.txt` and `holodisk.txt`
(SPEC-018 §4.6, §5.9), NPC schedules run in each critter's
`critter_p_proc`, quest scripts run in one manifest global script
(SPEC-024a §4.3), world-state toggles are object visibility
(`set_obj_visibility`, SPEC-005 §2.5). The front end is one typed IR shared
by Fallout 3 and New Vegas, so that the two speech-check systems, the two
condition dialects and the two package sets differ only in one module
each. The engine gaps (section 11) are workarounds, not blockers, except
the INT assembler, which `emit` provides locally until the brief lands.

## 1. Scope and target slice

### 1.1 The slice

| Area | Source cells | Notes |
|---|---|---|
| `megaton` | worldspace `MegatonWorld` (17 grid cells and its persistent cell) and 18 interiors whose editor id starts `Megaton` (one is a dummy holding cell; `MegatonAbandonedHouseDummyCell` is left out) | `MegatonMoriartysSaloon`, `MegatonLucasSimmsHouse`, `MegatonCratersideSupply` and the rest |
| `springvale` | `Wasteland` cells x -3..-1, y -5..-3 (around `Springvale` at (-2, -4), with `MegatonMainGate` and `SpringvaleBanditHouse`) | exterior only; `SpringvaleSchool*` is out |
| `sdm_exterior` | `Wasteland` cells x 3..5, y -6..-4 (around `SuperDuperMartExterior` at (4, -5)) | |
| `superdupermart` | interior `SuperDuperMart` | |

Persistent references of a worldspace are assigned to an area by their
position (`mir::cell_of`), as the map stage already does.

### 1.2 What "playable" means for Pass 2

1. **Talk.** Every named NPC in the slice answers the talk action with
   their greeting, a topic list, choices, and goodbye, in text, with no
   talking head. Choices respect conditions and say-once rules.
2. **Speech checks.** Fallout 3 percentage challenges show a chance, roll
   once, and take the success or the failure branch. Attribute and skill
   gated options appear only when met (section 4.7).
3. **Quests.** At least MS11 and the MQ01 stages reachable in Megaton, and
   the MS03 stages tied to the Super-Duper Mart, advance through dialogue,
   pickups and kills; their objectives show and complete in the Pip-Boy.
4. **Fights.** The raiders in `SuperDuperMart` attack on sight and fight
   with an engine AI packet; townsfolk only fight back.
5. **Reading.** Terminals open as a menu, including sub-menus, passwords
   and a hacking check; notes picked up or added by scripts can be read in
   the Pip-Boy.
6. **Locks.** Locked doors and containers open with the key, or with
   Lockpick at the right level.
7. **World changes.** Enable-parent groups switch on and off when a quest
   or a script says so, on the current map and on maps visited later.
8. **Schedules.** NPCs walk to their scheduled place, wander inside a
   radius, follow, guard or stand, by game time.

Out of scope for Pass 2: followers and companions, barter beyond opening
the engine's barter window, NPC-to-NPC conversations, combat and
detection barks, voice and lip sync, perks, spells and actor effects,
radiation from scripts, image spaces, achievements, map markers and
compass targets, creatures until their art exists, and every area outside
the slice.

### 1.3 Measured counts

All counts are from the owner's `Fallout3.esm`, read-only (appendix B).

**Whole plugin.** 1,257 `SCPT` (1,118 object, 101 quest, 38 effect;
1.41 MB of source, every one with source text), 6,381 `DIAL`, 22,327
`INFO`, 192 `QUST`, 3,266 `PACK`, 378 `TERM`, 840 `NOTE` (752 text, 67
voice, 18 image, 3 sound), 155 `GLOB`. The probe decodes every record with
no error class; `PACK`, `TERM` and `NOTE` are not typed yet, `INFO` has a
3-byte `DATA` form (634 records) and 16- and 20-byte `TRDT` forms (3,819
subrecords) that the typed decoder leaves in `unknown`.

**Placed references.**

| Area | REFR | ACHR | ACRE | Scripted instances | Teleport doors | Locked |
|---|---:|---:|---:|---:|---:|---:|
| `megaton` | 4,885 | 45 | 4 | 94 | 40 | 28 |
| `springvale` | 1,556 | 0 | 2 | 0 | 0 | 2 |
| `sdm_exterior` | 716 | 0 | 3 | 1 | 0 | 0 |
| `superdupermart` | 1,756 | 10 | 1 | 8 | 2 | 4 |

55 distinct actor bases, leveled entries included: Megaton 45 NPC and 4
creature bases (9 actors start disabled); `SuperDuperMart` 8 `LvlRaiderGun`
and 2 `LvlRaiderMelee` placements (NPC bases templated on leveled lists)
and one `genericOfficeProtectron`; the two exteriors hold only creatures.

**Dialogue.** 2,041 INFOs name a slice actor with `GetIsID == 1`, in 510
topics, for 26 speakers; 229 more are gated by the speakers' 22 voice types
alone. They carry 2,842 responses (at most 5 per INFO), 4,374 conditions
(112 OR-flagged; run on subject 3,744, target 589, reference 33, combat
target 8), 764 INFOs with choices (2,077 choice links), 323 link-from
entries, 379 player prompts, 536 result-script blocks in 518 INFOs, and no
`PNAM` order or `NAME` add-topic entries.

| By speaker (top) | INFOs | | By quest (top) | INFOs |
|---|---:|---|---|---:|
| `MoiraBrown` | 487 | | `DialogueMegaton` | 667 |
| `Jericho` | 222 | | `MS03` | 376 |
| `ColinMoriarty` | 165 | | `ConvMegaton` | 301 |
| `LucasSimms` | 155 | | `MS11` | 143 |
| `MisterBurke` | 139 | | `GenericCustomVoiceCombat` | 106 |
| `Gob` | 125 | | `MQ01` | 105 |
| `Nova` | 112 | | `Followers` | 67 |
| `DocChurch` | 98 | | `FreeformMegaton` | 52 |
| `ConfessorCromwell` | 88 | | `MS09`, `MS08`, `Doctors` | 45, 33, 30 |

Topic types of those INFOs: topic 1,222, conversation 616, combat 118,
detection 71, service 14. `GREETING` 236, `HELLO` 198, `GOODBYE` 109.
INFO flags: random 884, goodbye 292, random-end 234, say-once 100, info
refusal 92, say-once-a-day 56, run-immediately 37, speech challenge 22.

**Speech checks.** Plugin-wide, 263 INFOs carry the challenge flag (bit
`0x80` of the first flag byte) with a difficulty in `DNAM` 1 to 5 (40, 58,
95, 62, 8 by level) and a label actor value in `KNAM`. Each holds the
success content only; the failures live in the shared topic
`SpeechChallengeFailure` (187 INFOs, 23 for slice speakers), chosen by
speaker and quest conditions. `DNAM` also appears with 0 or 1 on about
1,070 ordinary INFOs, so the flag, not `DNAM`, marks a challenge. The plugin has
the game settings `iSpeechChallengeDifficulty{VeryEasy..VeryHard}`,
`fSpeechChallengeMultiplier`, `fSpeechAutoSuccessThreshold`,
`fSpeechChallengeFailure{Base,Multiplier}`, and XP rewards for hacking,
lock picking and speech. For comparison, New Vegas (`FalloutNV.esm`) uses
the flag on 3 INFOs only; its checks are condition pairs on the player's
actor value, for example Speech (43) `>=` 245 times and `<` 220 times,
Barter (32), Medicine (37), Science (40), Explosives (35).

**Conditions on slice dialogue, by function** (index, editor name from the
public function list): see the mapping table in section 4.8.

**Scripts.**

| Set | Sources | Bytes | Lines | Blocks |
|---|---:|---:|---:|---|
| Object scripts of slice bases | 67 | 73,713 | 2,822 | OnActivate 42, GameMode 21, OnDeath 20, OnLoad 4, SayToDone 4, OnTriggerEnter 3, OnTriggerLeave 2, OnTrigger 1, OnHit 1, OnStartCombat 1, OnCombatEnd 1, OnPackageStart/End/Change 1 each, MenuMode 1 |
| Quest scripts of the 27 slice quests | 19 | 90,868 | 2,451 | GameMode 18, MenuMode 3; 422 short, 13 float, 6 ref variables; none sets `fQuestDelayTime` |
| INFO result scripts | 536 | 48,635 | 1,727 | — |
| Quest-stage result scripts | 174 | 107,861 | 2,981 | — |
| Terminal item scripts | 7 | 258 | 11 | — |

21 object scripts with a GameMode block have 32 placed instances. Slice
scripts name 311 distinct placed references by editor id (199 `REFR`, 112
`ACHR`), of which 97 stand inside the slice; Megaton has 324 persistent
references with an editor id. They name 92 `MESG`, 87 `NOTE` and 46 `GLOB`
records. 196 distinct functions are called; the most used 20 cover 64 % of
calls, 40 cover 79 %, 60 cover 87 %, 80 cover 92 %. Most used: `SetStage`
319, `SetObjectiveDisplayed` 290, `RemoveItem` 148, `GetInCell` 143,
`AddNote` 133, `ShowMessage` 132, `GetItemCount` 124, `MoveTo` 116,
`SetObjectiveCompleted` 110, `GetObjectiveDisplayed` 106, `Disable` 104,
`AddItem` 103, `Enable` 88, `EvaluatePackage` 85, `Kill` 77,
`GetObjectiveCompleted` 71, `GetHasNote` 66, `SetAV` 54, `GetDead` 45,
`GetInWorldspace` 45, `GetInSameCell` 40, `IsActionRef` 38, `RewardXP` 35,
`GetStage` 33, `AddPerk` 32, `GetAV` 30, `RewardKarma` 28,
`ShowBarterMenu` 26.

**Quests involved** (stages / objectives): `MQ01` 44/8, `MS03` 34/34,
`CG00` 20/0, `FreeformMegaton` 13/0, `MS11` 10/12, `MS08` 10/8, `MS09`
9/10, `MQ09` 9/6, `MS12` 6/6, `MS14` 4/13, `Doctors` 4/0, and 16 small
dialogue holders. `MS11`'s stages are 0, 10, 15, 30, 40, 50, 60, 200, 210,
255.

**Packages of slice actors.** 54 of 55 actor bases have packages: 265
distinct, 281 slots. By type: Sandbox 83, Travel 61, Sleep 36, Eat 33,
Dialogue 22, Wander 12, Patrol 7, Follow 4, Guard 3, Accompany 2, Escort 1,
UseItemAt 1. 155 have an hour in their schedule, 110 run any time.
Location: near a reference 203, in a cell 19, near the current location
12, near the editor location 11, near a linked reference 3. Conditions:
`GetQuestVariable` 48, `GetScriptVariable` 25, `GetInCell` 21,
`GetRandomPercent` 20, `GetDead` 16, `GetCurrentTime` 13, `GetStage` 10,
`GetActorValue` 9, `GetItemCount` 9, `IsRaining` 6, others under 6.

**Enable parents.** 763 references in 18 groups (16 Megaton, 1
`SuperDuperMart`, 1 `sdm_exterior`): sizes 144, 140, 132, 105, 102, 91, 16,
9, 7 and smaller, 5 singletons. Parents: 17 marker statics and 1 NPC; 13
parents start disabled. One child has the opposite flag. No parent has a
parent of its own.

**Locks.** 34 locked references: levels 0 (6), 25 (3), 50 (3), 75 (4), 100
(5), key only 255 (5), and off-grid values 7, 20, 40, 80 (8 in all); 20
name a key.

**Terminals and notes.** 8 placed terminals (6 Megaton, 2
`SuperDuperMart`), 11 with sub-menus, 39 menu items (22 with conditions, 7
with a result script), 4 with a password note; base hacking difficulty
very easy 7, easy 2, average 1, hard 1. 18 placed notes (all text); slice
scripts add 60 distinct notes (49 text, 10 voice, 1 image).

**Trigger volumes.** 5 scripted activator primitives (3 Megaton, 1 each
Super-Duper Mart inside and out).

## 2. Engine facts this design rests on

Each was checked; this table is where the rest of the document cites from.

| # | Fact | Where |
|---|---|---|
| E1 | Procedure names and event numbers: `start` 1, `spatial_p_proc` 2, `description_p_proc` 3, `pickup_p_proc` 4, `use_p_proc` 6, `use_obj_on_p_proc` 7, `use_skill_on_p_proc` 8, `talk_p_proc` 11, `critter_p_proc` 12, `combat_p_proc` 13, `damage_p_proc` 14, `map_enter_p_proc` 15, `map_exit_p_proc` 16, `destroy_p_proc` 18, `look_at_p_proc` 21, `timed_event_p_proc` 22, `map_update_p_proc` 23. A missing procedure falls back to index 1 except in map broadcasts | SPEC-004 §3.7 |
| E2 | A host-called procedure must leave a result or placeholder above the saved context | SPEC-004 §3.6, SPEC-024a §4.3 |
| E3 | Imported cross-program calls are not reliable; do not use them | SPEC-004 §3.6 |
| E4 | Map load order: map script `map_enter_p_proc` first (fixed parameter 1 fresh, 0 saved), then every instance's `start`, then the `map_enter_p_proc` broadcast, then `map_update_p_proc` | SPEC-006 §3.5 |
| E5 | `map_update_p_proc` every 600 ticks (60 game seconds), and also after dialogue exit, elevation change and several completed uses | SPEC-006 §3.5, §3.7 |
| E6 | One `critter_p_proc` per background pass, round robin over the map's critter-category script records; never in combat or dialogue | SPEC-006 §3.7 |
| E7 | `add_timer_event(obj, ticks, param)` → `timed_event_p_proc` with that fixed parameter, one-shot; ten ticks a second | SPEC-005 §2.9, SPEC-006 §3.7 |
| E8 | Spatial scripts fire `spatial_p_proc` on every movement step within their radius (occupancy, not edges), source = mover; off during map-entry broadcasts; a teleport `move_to` does not trigger | SPEC-006 §3.6 |
| E9 | Talk needs a critter target with a script; dialogue entry refuses combat | SPEC-006 §3.8 |
| E10 | Dialogue words: `start_gdialog` (head -1 = no head), `gsay_start`, `gsay_reply`, `gsay_option`, `giq_option` (INT plus Smooth Talker against a threshold), `gsay_message`, `gsay_end`, `end_dialogue`, `gdialog_barter`; numeric callbacks only; 30 options; reply 899 bytes with paging; `-2/-2` is the Done label | SPEC-005 §2.7, §3.3; SPEC-006 §3.9, §3.10 |
| E11 | Message list k of the dialogue words is `text/<lang>/dialog/<stem>.msg` for line k-1 of `scripts/scripts.lst`; a row registered only by `[[scripts]]` has **no** dialogue file | SPEC-005 §3.3, SPEC-006 §3.9; `vault_core::assets` (`dialogue_path` reads `scripts.lst` only) |
| E12 | `local_var`/`set_local_var` (per instance, count from the `scripts.lst` row's `local_vars=`), `map_var`, `global_var`; values are integers | SPEC-005 §2.4, SPEC-003 |
| E13 | Fresh maps do not take script locals from the map file: each record's stored count is reset and slices are zero-allocated lazily | SPEC-003 (script records, local pool) |
| E14 | Campaign globals come from `data/vault13.gam`; under Fallout 2 none has an engine meaning; the count is whatever the file allocates | SPEC-006 §2.1, SPEC-018 §6.2 |
| E15 | Pip-Boy: `quests.txt` rows (location msg in `map.msg`, description msg in `quests.msg`, global, display and completed thresholds) and `holodisk.txt` rows (global > 0 acquires; title and a 500-id text window in `pipboy.msg`; 35 lines a page; `**END-PAR**`, `**END-DISK**`) | SPEC-018 §4.6, §5.9; SPEC-008 §3.8; SPEC-014 §3.41.2 (font 101, 10-pixel lines, text area 374 px wide) |
| E16 | Exported variables: per-map registry of 1,013 names, cleared at every map load, re-created by the first exporter; a missing name is a VM error | SPEC-005 §2.2 |
| E17 | Visibility: `set_obj_visibility(obj, 1)` hides (critters also become non-blocking); hidden objects block nothing; map object flag `0x01` is hidden | SPEC-005 §2.5, §3.4; SPEC-007 (blocker query); SPEC-003 |
| E18 | `tile_contains_pid_obj(hex, elev, pid)` finds hidden objects too (no hidden filter; owned items excluded) | `vault_core::host_game` |
| E19 | Movement: `animate_move_obj_to_tile` (walk or run, outside combat), `reg_anim_*` sequences, `anim_busy`, `anim(obj, 1000, dir)` for facing, `reg_anim_turn_towards` (Fallout 2), `move_to` (instant) | SPEC-005 §2.6, SPEC-017 §4.1 |
| E20 | Skills and stats: `has_skill` (level, 0-17, Fallout 2 numbering), `roll_vs_skill`, `is_success`, `get_critter_stat`, `get_pc_stat` (4 = karma, read only), `random(min, max)`, `give_exp_points` | SPEC-005 §2.8, §3.5; SPEC-006 §3.3, §3.4 |
| E21 | No engine lockpick, science or repair effect: locks and computers work only through `use_skill_on_p_proc` scripts and `obj_lock`/`obj_unlock`/`obj_is_locked`; jammed locks refuse every skill before the script | SPEC-013 §3.3, §3.4, §3.8, §3.14 |
| E22 | Lock bits: container `0x02000000` locked, `0x04000000` jammed; door state word likewise | SPEC-003 |
| E23 | Combat: no AI packet makes a critter hostile; a script starts combat with `attack_setup`; teams via `critter_add_trait(obj, 1, 6, team)`; packets from `data/ai.txt` | SPEC-009 §3.1, §3.15, §3.18; contracts 10.1 |
| E24 | Exit grids move the player; ladders and stairs move only through `use_p_proc`; three elevations | SPEC-013 §3.22, §3.23 |
| E25 | `load_map` under Fallout 2 loads at the header's default entrance (entrance parameter stored nowhere); `override_map_start` moves the player at once | SPEC-017 §3.1, SPEC-018 §5.1, SPEC-005 §2.9 |
| E26 | Manifest scripting: `[[global_scripts]]` (`start` once at setup and every `repeat` steps; `map_enter/exit/update_p_proc` at those events; no attached object), `[[hooks]]` on 30 points, words `0x8300..=0x8309`; every mod `.int` passes the SPEC-017 §8 stack model or is refused; 1,000,000-instruction cap per host call | SPEC-024a §2-§7, REFERENCE "Mod-script verification" |
| E27 | A step is one frame of the session pipeline, not a game tick; `repeat` counts steps | SPEC-021 §3.10, SPEC-024b §4.9, §5.3 |
| E28 | `message_box`, `create_win` and sfall arrays exist only in sfall mode, which a layer may switch on with `[scripting] sfall = true`; script windows are not drawn | SPEC-024b §3.1, §4.11, D-49 |
| E29 | String `add` concatenates; `message_str(list, id)` returns text; options accept literal or dynamic strings | SPEC-004 (arithmetic), SPEC-005 §2.10, §3.3 |
| E30 | Karma is a progression value (`get_pc_stat` 4), not a global; no word writes it | SPEC-019 §3.9, SPEC-011, SPEC-005 §3.5 |
| E31 | No terminal or document reader UI; ACM-only audio | SPEC-014, SPEC-010 |

## 3. The IR (`transpile::ir`)

The IR is plain data (`serde`, `BTreeMap`, sorted `Vec`), built from MIR,
checked, then lowered. It never holds engine numbers; `emit` allocates
those (section 10).

### 3.1 Identifiers

```rust
/// Every IR object is named by the source form it came from.
pub struct FormKey(pub String);           // "fallout3.esm:01a2b3", mir's form identity

/// An IR identifier: kind plus form plus a sub-index where one form makes
/// several objects (a quest's stage 10, a terminal's menu item 2).
#[derive(Ord, PartialOrd, Eq, PartialEq, Clone)]
pub struct SimId { pub kind: SimKind, pub form: FormKey, pub sub: u32 }
pub enum SimKind { Topic, Info, Quest, Stage, Objective, QuestVar, Global,
                   Script, ScriptVar, Package, Group, Terminal, MenuItem,
                   Note, Actor, Ref, Text }
```

Every lowered artifact (procedure name, message id, global index) is
derived from a `SimId`, so the report and the debug overlay can name the
source form of any number. Order everywhere is `SimId` order.

### 3.2 Text

```rust
pub struct TextId(pub u32);               // index into Module::texts
pub struct Text { pub owner: SimId, pub source: String, pub role: TextRole }
pub enum TextRole { Response, Prompt, TopicName, QuestName, Objective,
                    LogEntry, TerminalHeader, MenuItem, MenuResult,
                    NoteTitle, NoteBody, MessageBox, Label }
```

Texts are kept as decoded (Windows-1252) and converted once in `emit`
(section 4.10). The converter's own labels ("[Continue]", the skill-check
brackets) are `TextRole::Label` with `owner` the label's id; they are the
only text this repository writes.

### 3.3 Expressions and conditions

```rust
pub enum Expr {
    Int(i32), Float(f32), Str(TextId),
    Var(VarRef),
    Form(FormKey),                        // a base form: becomes a PID, a msg id, ...
    Ref(RefExpr),
    Call(Box<Call>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}
pub enum RefExpr { Player, SelfRef, Action, Placed(FormKey), Var(VarRef), Linked(Box<RefExpr>) }
pub struct Call { pub func: Func, pub on: Option<RefExpr>, pub args: Vec<Expr> }
pub enum Func { Known(FuncId), Unsupported { name: String } }

pub enum VarRef {
    Local  { script: SimId, index: u32, ty: VarTy },   // object-script variable
    Quest  { quest: SimId, index: u32, ty: VarTy },    // quest-script variable
    Global(SimId),                                     // GLOB
    Hoisted { owner: FormKey, script: SimId, index: u32, ty: VarTy },
    Temp(u32),                                         // procedure-local
}
pub enum VarTy { Int, Float, Ref }

pub enum Cond {
    True, False,
    Not(Box<Cond>), All(Vec<Cond>), Any(Vec<Cond>),
    Cmp { lhs: Expr, op: CmpOp, rhs: Expr },
    Fallback { function: u16, name: String, value: f32, report: bool },
}
```

A `CTDA` list becomes one `Cond` by the editor's rule: a condition with the
OR flag is OR-ed with the next one, and the OR runs are AND-ed, so
`[A or, B, C or, D]` is `(A ∨ B) ∧ (C ∨ D)`. The comparison value is a
literal or a `GLOB` (`ConditionValue::Global` → `Expr::Var(Global)`).
`run_on` becomes the call's `on`: 0 subject, 1 target, 2 the condition's
reference, 3 combat target, 4 linked reference.

### 3.4 Dialogue

```rust
pub struct Topic {
    pub id: SimId, pub editor_id: String, pub kind: TopicKind,
    pub name: Option<TextId>, pub priority: f32, pub top_level: bool,
    pub infos: Vec<SimId>,                // file order inside the topic group
}
pub enum TopicKind { Topic, Conversation, Combat, Persuasion, Detection,
                     Service, Misc, Radio }

pub struct Info {
    pub id: SimId, pub topic: SimId, pub quest: SimId,
    pub speakers: SpeakerSet,             // section 4.2
    pub condition: Cond,                  // speaker terms folded out
    pub flags: InfoFlags,
    pub responses: Vec<Response>,
    pub prompt: Option<TextId>,           // RNAM; else the topic name
    pub choices: Vec<SimId>,              // TCLT, in order
    pub link_from: Vec<SimId>,            // TCLF
    pub add_topics: Vec<SimId>,           // NAME
    pub begin: Option<Block>, pub end: Option<Block>,   // before / after NEXT
    pub check: Option<SkillCheck>,
}
pub struct InfoFlags { pub goodbye: bool, pub random: bool, pub random_end: bool,
                       pub say_once: bool, pub say_once_a_day: bool,
                       pub run_immediately: bool, pub info_refusal: bool }
pub struct Response { pub number: u8, pub text: TextId, pub emotion: (u32, i32) }

pub enum SpeakerSet { Bases(Vec<FormKey>), Any }

pub enum SkillCheck {
    /// Fallout 3: a chance shown on the option, one roll, two branches.
    Percent { skill: Skill, difficulty: u8, label: Option<ActorValue>,
              failure: FailureRoute, remember: Remember },
    /// New Vegas, and Fallout 3 attribute/skill gates: a comparison, no roll.
    Threshold { stat: Stat, op: CmpOp, value: i32, label: Option<ActorValue>,
                shown_when_failed: bool, partner: Option<SimId> },
}
pub enum FailureRoute { SharedTopic(SimId), NextInfo }
pub enum Remember { PerSelection, Forever }
pub enum Stat { Skill(Skill), Special(u8) }
pub enum Skill { SmallGuns, BigGuns, EnergyWeapons, Unarmed, Melee, Throwing,
                 FirstAid, Doctor, Sneak, Lockpick, Steal, Traps, Science,
                 Repair, Speech, Barter, Gambling, Outdoorsman }   // Fallout 2 order
```

`Skill` is in Fallout 2 order (SPEC-006 §3.3) because it is a lowering
target; the source actor value maps to it in `dialogue::checks` (section
4.7).

### 3.5 Scripts

```rust
pub struct Script {
    pub id: SimId, pub editor_id: String, pub kind: ScriptKind,
    pub vars: Vec<VarDecl>, pub blocks: Vec<Block>, pub diagnostics: Vec<Diag>,
}
pub enum ScriptKind { Object, Quest, Effect, Result(ResultOwner) }
pub enum ResultOwner { InfoBegin(SimId), InfoEnd(SimId), Stage(SimId),
                       MenuItem(SimId), PackageEvent(SimId, PackageEvent) }
pub struct VarDecl { pub index: u32, pub name: String, pub ty: VarTy }
pub struct Block { pub event: Event, pub body: Vec<Stmt> }
pub enum Event {
    GameMode, MenuMode, OnActivate, OnLoad, OnDeath { killer: Option<RefExpr> },
    OnHit { by: Option<RefExpr> }, OnTrigger { by: Option<RefExpr> },
    OnTriggerEnter { by: Option<RefExpr> }, OnTriggerLeave { by: Option<RefExpr> },
    OnStartCombat, OnCombatEnd, SayToDone, OnPackageStart(Option<FormKey>),
    OnPackageEnd(Option<FormKey>), OnPackageChange(Option<FormKey>),
    OnAdd, OnEquip, OnUnequip, ScriptEffectStart, ScriptEffectUpdate,
    ScriptEffectFinish, Result, Other(String),
}
pub enum Stmt {
    Set(VarRef, Expr),
    If { arms: Vec<(Expr, Vec<Stmt>)>, otherwise: Vec<Stmt> },
    Call(Call),
    Return,
}
```

### 3.6 Quests

```rust
pub struct Quest {
    pub id: SimId, pub editor_id: String, pub name: Option<TextId>,
    pub start_enabled: bool, pub priority: u8,
    pub delay_seconds: Option<f32>,       // DATA delay; None or 0 = default
    pub script: Option<SimId>,
    pub vars: Vec<VarDecl>,               // the quest script's variables
    pub stages: Vec<Stage>, pub objectives: Vec<Objective>,
}
pub struct Stage { pub id: SimId, pub index: i16, pub entries: Vec<LogEntry> }
pub struct LogEntry { pub complete: bool, pub fail: bool, pub condition: Cond,
                      pub text: Option<TextId>, pub script: Option<SimId>,
                      pub next_quest: Option<SimId> }
pub struct Objective { pub id: SimId, pub index: i32, pub text: Option<TextId>,
                       pub targets: Vec<(FormKey, Cond)> }   // kept, not lowered
```

### 3.7 Packages

```rust
pub struct Package {
    pub id: SimId, pub editor_id: String, pub kind: PackageKind,
    pub flags: u32, pub behaviour_flags: u16, pub type_flags: u16,
    pub location: Option<Location>, pub target: Option<Target>,
    pub schedule: Schedule, pub condition: Cond,
    pub events: Vec<(PackageEvent, Option<SimId>)>,   // begin, end, change scripts
    pub runner: Runner,
}
pub enum PackageKind { Find, Follow, Escort, Eat, Sleep, Wander, Travel,
                       Accompany, UseItemAt, Ambush, FleeNotCombat, Sandbox,
                       Patrol, Guard, Dialogue, UseWeapon, Other(u8) }
pub enum Location { NearRef { r: FormKey, radius: i32 }, InCell { cell: FormKey, radius: i32 },
                    NearCurrent { radius: i32 }, NearEditor { radius: i32 },
                    ObjectId(FormKey), ObjectType(u32), NearLinkedRef { radius: i32 },
                    AtPackageLocation }
pub enum Target { Ref(FormKey), Object(FormKey), ObjectType(u32), LinkedRef }
pub struct Schedule { pub month: i8, pub day_of_week: i8, pub date: u8,
                      pub hour: i8, pub duration_hours: i32 }   // -1 = any
pub enum PackageEvent { Begin, End, Change }
pub enum Runner { Travel, Sandbox, Follow, Guard, Wait, None }
```

Type numbers as stored (verified by editor-id pattern on the slice):
0 Find, 1 Follow, 2 Escort, 3 Eat, 4 Sleep, 5 Wander, 6 Travel, 7
Accompany, 8 UseItemAt, 9 Ambush, 10 FleeNotCombat, 12 Sandbox, 13
Patrol, 14 Guard, 15 Dialogue, 16 UseWeapon.

### 3.8 World state, terminals, notes, references

```rust
pub struct EnableGroup { pub id: SimId /* the parent */, pub initially_enabled: bool,
                         pub children: Vec<(FormKey, bool /* opposite */)> }

pub struct Terminal {
    pub id: SimId, pub header: Option<TextId>,
    pub difficulty: u8,                   // 0 very easy .. 4 very hard, 5 key only
    pub flags: u8, pub server: u8,
    pub password: Option<SimId>,          // a Note
    pub items: Vec<MenuItem>,
}
pub struct MenuItem { pub id: SimId, pub text: TextId, pub result: Option<TextId>,
                      pub add_note: bool, pub show_note: Option<SimId>,
                      pub submenu: Option<SimId>, pub script: Option<SimId>,
                      pub condition: Cond }

pub struct Note { pub id: SimId, pub title: Option<TextId>, pub body: NoteBody }
pub enum NoteBody { Text(TextId), Voice { topic: Option<SimId> }, Image(String), Sound(String) }

/// Every placed reference something needs at run time (section 5.8).
pub struct RefEntry { pub form: FormKey, pub editor_id: Option<String>,
                      pub base: FormKey, pub area: String, pub mobile: bool,
                      pub why: RefUses }      // bit set: script, condition, package,
                                              // quest target, enable parent, linked
```

### 3.9 The module

```rust
pub struct Module {
    pub texts: Vec<Text>,
    pub topics: BTreeMap<SimId, Topic>, pub infos: BTreeMap<SimId, Info>,
    pub scripts: BTreeMap<SimId, Script>, pub quests: BTreeMap<SimId, Quest>,
    pub globals: BTreeMap<SimId, Global>, pub packages: BTreeMap<SimId, Package>,
    pub groups: BTreeMap<SimId, EnableGroup>, pub terminals: BTreeMap<SimId, Terminal>,
    pub notes: BTreeMap<SimId, Note>, pub refs: BTreeMap<FormKey, RefEntry>,
    pub actors: BTreeMap<FormKey, Actor>,     // base: script, packages, factions, AI data
    pub diagnostics: Vec<Diag>,               // the report's source
}
```

`transpile::build(mir: &mir::Mir, slice: &SimSlice, config: &SimConfig)
-> Result<Module, TranspileError>` fills it; `transpile::lower(&Module,
&Placement, &BaseFiles) -> Result<Lowered, TranspileError>` turns it into
programs and tables (section 10). `Placement` is what the map stage built:
for each placed reference, its map, hex, elevation, PID and object id.

## 4. Dialogue compiler (`transpile::dialogue`)

### 4.1 Source semantics

As the editor presents them, and as this compiler reproduces them:

- A **topic** holds INFOs in order. Choosing a topic says the **first
  INFO whose conditions pass** for this speaker. A run of INFOs flagged
  random, up to and including one flagged random-end, is one random group:
  one is drawn among those that pass.
- An INFO's responses are said in order; then its `end` script runs; then
  the player chooses again.
- **Choices** after an INFO are its `TCLT` topics, plus every topic with
  an INFO whose `TCLF` names the current topic (**calibration**: the
  reverse-link reading), each shown only if one of its INFOs passes. An
  INFO with no choices returns to the **hub**: the top-level topics
  (`DIAL` flag `0x02`) and topics the player has been given with
  `AddTopic`, each shown if an INFO passes.
- **Option text** is the passing INFO's prompt, else the topic's name.
- **Greeting**: talking says the first passing INFO of `GREETING`.
- **Goodbye** on an INFO ends the conversation after it.
- **Say once**: never again after being said. **Say once a day**: not
  again the same game day. **Info refusal**: said, but the player stays in
  the same choice list.
- Topic order in a list: priority descending, then the order of first
  appearance (choice order, then topic form order for the hub).

### 4.2 Speakers and static folding

The compiler emits one dialogue per **speaker base** (an `NPC_` or `CREA`
placed in the slice). For each base it collects the INFOs that can pass
for it and folds every condition term that depends only on the speaker's
base, at compile time:

| Term | Folded with |
|---|---|
| `GetIsID` on subject | base equality |
| `GetIsVoiceType`, `GetIsSex`, `GetIsRace`, `GetIsClass`, `GetIsCreature` on subject | the base record (after templates) |
| `GetInFaction` on subject | the base's factions, unless some script adds or removes that faction for this base (then a runtime term) |
| `GetIsReference` on subject | true only when the base has one placement |

An INFO whose folded condition is `False` is dropped for that speaker.
Measured effect: 2,041 `GetIsID` INFOs and 229 voice-type INFOs become 26
per-speaker dialogues; `GREETING`'s 2,542 INFOs shrink to 236.

### 4.3 The node model in the engine

Fallout 2 dialogue is a reply plus options, each option a numeric callback
procedure; a callback that builds a new reply continues, one that builds
none ends the loop (E10). The compiler maps:

| Source | Engine |
|---|---|
| Talk | `talk_p_proc`: `start_gdialog(list, self_obj, 4, -1, -1)` (no head), `gsay_start`, call the greeting node, `gsay_end`, `end_dialogue` |
| An INFO said | node procedure `i_<n>`: run `begin`; `gsay_reply` with the responses; options |
| Responses | joined with one space into one reply while the result fits 899 bytes; otherwise a chain of replies, each with one "[Continue]" option to the next part (the engine pages long replies itself, E10) |
| A choice | `gsay_option(list, text, o_<slot>, 50)`, or `giq_option` (section 4.7) |
| Option callback `o_<slot>` | run the previous INFO's `end` block (once), then say the INFO picked for that slot |
| Goodbye | the INFO's options are one `gsay_option(-2, -2, x_end, 50)` ("Done" label); `x_end` runs `end` and builds nothing |
| Hub | procedure `hub`: options for the hub topics, plus a converter "Goodbye" label option |
| End of talk | after `gsay_end`: run a pending `end` block, then `end_dialogue` |

The `end` block of the INFO just said runs when the player leaves its
reply (option callback or Done), which is when the source runs it. With
`run_immediately` the `end` block runs right after `begin` instead.

### 4.4 Generated procedures, per speaker

- `p_<t>()` → INFO ordinal or 0: the pick for topic t, honouring order,
  random groups (one `random(1, n)` over the passing members), say-once
  and say-once-a-day. Pure: no side effect except the random draw.
- At option-building time the pick is made once and stored in a program
  variable `k_<slot>`; the callback says that INFO, so the option shown is
  the INFO said even with random groups.
- `i_<n>()`: say INFO n (mark say-once, the talked-to flag, then 4.3).
- `o_<slot>()`: option callbacks, one per option slot (at most 30).
- `c_<n>()`: INFO n's condition as a predicate, shared by the picks.

Counts for the slice: at most 487 node procedures for one speaker
(`MoiraBrown`); the loader allows 65,536.

### 4.5 State the dialogue keeps

| State | Where |
|---|---|
| Talked to the player (`GetTalkedToPC`) | speaker script local `L_TALKED` |
| Say once | one campaign-global bit per INFO (31 bits a global) |
| Say once a day | one campaign global per INFO: the game day said (`game_time / 864000`) |
| Topics given by `AddTopic` | one global bit per topic |
| Challenge outcome | two global bits per challenge INFO: attempted, passed |
| Pending `end` block | program variable (one conversation at a time) |

Measured: 100 say-once and 56 say-once-a-day INFOs in the slice, 5
`AddTopic` calls, 22 challenges.

### 4.6 Barks and other topic kinds

`HELLO`, combat, detection and conversation topics are not lowered in
Pass 2; the report counts them. A later pass can say `HELLO` with
`float_msg` from `critter_p_proc` when the player is within a few hexes.

### 4.7 Skill checks

One IR, two lowerings.

**Fallout 3 percentage (`SkillCheck::Percent`).** Built from an INFO with
the challenge flag: `difficulty` from `DNAM`, `label` from `KNAM`,
`failure = SharedTopic(SpeechChallengeFailure)`, `skill = Speech`.

- Shown: the option text is the label bracket, the chance and the prompt:
  `message_str(list, L_SPEECH) + chance + message_str(list, L_PCT) +
  prompt` (E29), so the chance is computed when the list is built.
- Chance (provisional, **calibration C-2**):
  `chance = clamp(round(S * M) - D[difficulty], 0, 100)` with
  `S = has_skill(dude_obj, 14)`, `M = fSpeechChallengeMultiplier` and
  `D[k] = iSpeechChallengeDifficulty<k>` read from the owner's plugin at
  conversion time, `100` when `S >= fSpeechAutoSuccessThreshold`. The
  formula lives in one function, `dialogue::checks::fo3_chance`, pinned by
  a test; calibrating it means comparing the original game's displayed
  percentage at two Speech values per difficulty.
- Chosen: if the challenge was attempted before and `remember` is
  `Forever`, reuse the stored outcome; else draw `random(1, 100) <= chance`
  once and store attempted and passed bits.
- Passed: say the challenge INFO (its responses and scripts) and give the
  plugin's speech XP for that difficulty with `give_exp_points`.
- Failed: say the first passing INFO of the failure route
  (`SpeechChallengeFailure`, speaker-folded like any topic), with its own
  script and choices; if none passes, say nothing and return to the
  current list.

**Threshold (`SkillCheck::Threshold`).** Built from a condition that
compares the player's (`run_on` target) attribute or skill against a
constant: `GetActorValue`, `GetBaseActorValue`, `GetPermanentActorValue`
with an actor value that maps to a `Stat`.

- Fallout 3: the gate only hides the option (`shown_when_failed = false`).
  When the gate is the INFO's only player-stat term and it is
  Intelligence `>= n`, the option is emitted as `giq_option(n, ...)`
  (E10), which also gives Smooth Talker its Fallout 2 meaning. Otherwise
  the gate is part of the pick condition.
- New Vegas: the pass and fail variants are INFOs of one topic with
  opposite comparisons (`>=` / `<`). They become one option
  (`shown_when_failed = true`, `partner` links them) whose label reads
  "[Speech 50]" when met and "[Speech 25/50]" when not, built like the
  percent label; choosing it says the first passing variant. No roll.
- Value scale: Fallout 3 and New Vegas skills run 0 to 100; Fallout 2
  skills reach 200 (SPEC-006 §3.3). Thresholds are compared 1:1 (open
  question 2).

Actor value to engine read (actor value numbers as stored, confirmed by
the New Vegas measurement):

| Source actor value | Engine read |
|---|---|
| 5-11 Strength .. Luck | `get_critter_stat(dude_obj, 0..6)` |
| 16 Health | `get_critter_stat(obj, <current hit points>)`; the stat number from SPEC-003's stat table (verify) |
| 23 Karma | `get_pc_stat(4) + G_KARMA`, the engine's value plus the shadow global (section 6.4) |
| 32 Barter | `has_skill(obj, 15)` |
| 33 Big Guns, 34 Energy Weapons, 41 Small Guns | `has_skill` 1, 2, 0 |
| 35 Explosives | `has_skill(obj, 11)` (Traps) |
| 36 Lockpick | `has_skill(obj, 9)` |
| 37 Medicine | `has_skill(obj, 7)` (Doctor) |
| 38 Melee Weapons, 45 Unarmed | `has_skill` 4, 3 |
| 39 Repair, 40 Science | `has_skill` 13, 12 |
| 42 Sneak, 43 Speech | `has_skill` 8, 14 |
| 25-30 body-part condition | fallback 100 |
| others (54, 62 on the speaker, 9 times) | fallback, reported |

### 4.8 Condition functions

The functions used by slice dialogue, by count, and what each lowers to.
"Static" means folded at compile time (section 4.2). The same table
serves package and stage conditions.

| Index | Function | Count | Lowering |
|---:|---|---:|---|
| 72 | `GetIsID` | 2,461 | static on subject; else `obj_pid(on) == PID` |
| 79 | `GetQuestVariable` | 979 | `global_var(G)` of the quest variable (section 5.4) |
| 58 | `GetStage` | 141 | `global_var(G_stage)` |
| 14 | `GetActorValue` | 126 | section 4.7 table |
| 47 | `GetItemCount` | 112 | `obj_is_carrying_obj_pid(on, PID)`; an unconverted item is 0 |
| 67 | `GetInCell` | 91 | `cur_map_index == map(cell)`; an exterior cell is the map window that holds it |
| 50 | `GetTalkedToPC` | 85 | speaker local `L_TALKED` |
| 59 | `GetStageDone` | 79 | stage-done bit of the quest (section 6.1) |
| 131 | `GetPCIsSex` | 49 | `get_critter_stat(dude_obj, <gender>)` (SPEC-003 stat number) |
| 53 | `GetScriptVariable` | 45 | the variable's storage (section 5.4) |
| 70 | `GetIsSex` | 21 | static on subject; else `get_critter_stat(on, <gender>)` |
| 182 | `GetEquipped` | 20 | `critter_inven_obj(on, 0/1/2)` PID compare |
| 74 | `GetGlobalValue` | 18 | `global_var(G)` |
| 71 | `GetInFaction` | 17 | static, or a faction-override global (section 4.2) |
| 161 | `GetIsCurrentPackage` | 16 | runner local `L_PKG == ordinal` |
| 431 | `GetHealthPercentage` | 14 | `100 * current / maximum hit points` (`get_critter_stat`, SPEC-003 stat numbers) |
| 1 | `GetDistance` | 13 | `tile_distance_objs(a, b) * 70`: a hex is half a floor square of 140 source units (contracts section 4) |
| 35 | `GetDisabled` | 9 | group state (section 8) or `obj_is_visible_flag` |
| 446 | `GetInZone` | 9 | fallback 0 |
| 300 | `IsInInterior` | 7 | static per map |
| 84 | `GetDeadCount` | 6 | per-base kill counter global, raised in `destroy_p_proc` |
| 117 | (index 117) | 6 | fallback 0, reported |
| 255 | `GetOffersServicesNow` | 6 | fallback 1 |
| 449 | `HasPerk` | 6 | fallback 0 |
| 492 | `GetMapMarkerVisible` | 6 | fallback 0 |
| 56 | `GetQuestRunning` | 5 | running bit of the quest |
| 289 | `IsInCombat` | 5 | `combat_is_initialized` |
| 46 | `GetDead` | 4 | `critter_state(on) & 1` on this map; the actor's dead global elsewhere |
| 76 | `GetDisposition` | 4 | fallback 50 |
| 415 | `Exists` | 3 | fallback 1 |
| 403 | `HasFriendDisposition` | 2 | fallback 1 |
| 18 | `GetCurrentTime` | 2 | `game_time_hour` as hours (`hhmm / 100 + (hhmm % 100) / 60`) |
| 77 | `GetRandomPercent` | 2 | `random(0, 99)` |
| 382 | `GetHasNote` | 2 | the note's holodisk global `> 0` |
| 515, 49, 323 | `IsCombatTarget`, `GetSleeping`, `WhichServiceMenu` | 1 each | fallback 0 |

Package conditions add `IsRaining` (fallback 0), `GetInWorldspace` (map
set), `GetInSameCell` (same map and elevation), `GetIsReference`
(reference equality) and `GetDetected` (`obj_can_see_obj`).

### 4.9 Fallback policy

An unsupported function is never silently true. It becomes
`Cond::Fallback` with a **value** chosen per function (the table above),
and the comparison is then evaluated normally, so `HasPerk X == 0` passes
and `HasPerk X == 1` fails. Functions with no table entry take value 0.
Every fallback writes one report line: function, index, count, and the
first owners by editor id. A dialogue whose every greeting INFO folds to
`False` gets a converter greeting label so that talk still opens the hub,
and is reported.

### 4.10 Message files

One file per speaker program, `text/english/dialog/<stem>.msg`, where
`<stem>` is the program's `scripts.lst` name (E11). Numbering:

| Ids | Content |
|---|---|
| 1-99 | converter labels (Continue, Goodbye, skill brackets, percent sign) |
| 10000 + 10 n + r | response r (0-8) of the speaker's INFO ordinal n |
| 10000 + 10 n + 9 | INFO n's prompt |
| 900000 + t | topic t's name |

Ids 970-1099 are left free: the engine's Ask About lookup reads them
(SPEC-006 §3.9). Text is written through `emit`'s MSG adapter: braces,
which the format cannot carry, become parentheses; characters outside
Windows-1252 become `?`; each change is counted in the report.

### 4.11 Voice

Text only. The response's sound field stays empty, so the engine requests
no speech (SPEC-006 §3.9). Voice files, lip data and talking heads belong
to the audio brief and a later pass.

## 5. Script compiler (`transpile::obscript`)

### 5.1 Input

`SCPT` source text (`ScriptBody::source`, present on all 1,257), the
embedded result scripts of `INFO`, quest stages, terminal items and
package events, and their `SCRO`/`SCRV` reference lists and `SLSD`/`SCVR`
variable declarations. The compiled `SCDA` bytes are not used.

### 5.2 Lexer and parser

- Lines; `;` starts a comment outside a string; keywords and identifiers
  are case-insensitive (the slice has `GameMode`, `gamemode`, `gameMode`,
  `Gamemode`).
- Grammar:

```text
script   := header? decl* block*
header   := ("scn" | "scriptname") IDENT
decl     := ("short" | "int" | "long" | "float" | "ref" | "reference") IDENT
block    := "begin" IDENT arg* NL stmt* "end"
stmt     := "set" lvalue "to" expr | if | "return" | call
if       := "if" expr NL stmt* ("elseif" expr NL stmt*)* ("else" NL stmt*)? "endif"
call     := (ref ".")? IDENT arg* ("," arg)*
expr     := or ; or := and ("||" and)* ; and := cmp ("&&" cmp)*
cmp      := sum (("==" | "!=" | "<" | "<=" | ">" | ">=") sum)?
sum      := term (("+" | "-") term)* ; term := unary (("*" | "/") unary)*
unary    := ("-" | "!")? atom
atom     := NUMBER | STRING | lvalue | call-in-expression | "(" expr ")"
lvalue   := IDENT | IDENT "." IDENT          // local, quest.var, ref.var
```

- A call inside an expression takes as many arguments as its signature in
  `obscript::functions` says. Unknown functions take atoms until an
  operator or line end, with a warning.
- Name resolution, in order: local variable, quest editor id (for
  `quest.var`), `player`/`playerref`, placed-reference editor id, base or
  other form editor id, function name. Forms resolve through the script's
  `SCRO` list where the name is ambiguous.
- Malformed source is a diagnostic for that script, never a panic; the
  script is reported unsupported and the rest of the build goes on.

### 5.3 Supported subset

Blocks (slice object scripts; quest scripts are GameMode):

| Block | Lowers to | Pass 2 |
|---|---|---|
| `OnActivate` (42) | objects: `use_p_proc`; critters: inside `talk_p_proc` before dialogue (section 5.5) | yes |
| `GameMode` (21 + 18 quest) | section 5.6 | yes |
| `OnDeath` (20) | `destroy_p_proc` | yes |
| `OnLoad` (4) | `map_enter_p_proc` | yes |
| `OnTriggerEnter`, `OnTriggerLeave`, `OnTrigger` (6) | spatial script (section 5.7) | yes |
| `OnHit` (1) | `damage_p_proc` | yes |
| `OnStartCombat`, `OnCombatEnd` (2) | `combat_p_proc` with an in-combat flag local | yes |
| `OnPackageStart`, `OnPackageEnd`, `OnPackageChange` (3) | called by the runner (section 7) | yes |
| `SayToDone` (4), `MenuMode` (4) | — | no |
| `OnAdd`, `OnEquip`, `ScriptEffect*` | — | no (items and effects) |

Functions: the 80 most used in the slice cover 92 % of calls. Pass 2
supports the lowering table in section 5.9; everything else is an
unsupported call, dropped with a report line (function, count, owners).
A block whose only statements are unsupported is reported as such.

### 5.4 Variables and storage

| Source | Engine storage | Notes |
|---|---|---|
| Object-script variable | the instance's local (E12) | `scripts.lst` `local_vars=` counts declared plus system locals |
| Object-script variable read or written from outside its script (`ref.var`, `GetScriptVariable` on another reference) | a campaign global (**hoisted**) | allowed only for a persistent reference with one placement; otherwise unsupported |
| Quest variable | one campaign global | only variables some compiled code reads or writes |
| `GLOB` | one campaign global, initialised from `FLTV` | |
| `float` anywhere persistent | integer, fixed point × 1000, rounded | in-procedure temporaries stay binary32 |
| `ref` anywhere persistent | the reference's slot number (section 5.8), 0 for none | |

System locals of every object program, first: `L_SLOT` (reference slot),
`L_INIT`, `L_TALKED`, `L_FLAGS` (in combat, inside trigger, hostile),
`L_LAST_GM` (last GameMode tick); critter programs add the runner's eight
(section 7.4). Initial values are set by the program on its first `start`
(`L_INIT == 0`), because fresh maps do not load locals from the map file
(E13).

Measured: 441 quest variables declared in the 19 quest scripts, 110 in
object scripts. With pruning, quest state, groups, notes and once-flags,
the slice needs about 700 to 900 new globals (section 10.2).

How a layer adds globals: the campaign array is exactly what
`data/vault13.gam` allocates (E14), with no engine limit stated in SPEC-006
§2.1 or SPEC-018 §6.2; the manifest's `[[patches]]` tables (critical,
critical_failure, terrain, frequency, region, towns, hotspots, maps;
REFERENCE "Mod manifests") have no globals table. So the layer ships a
whole `vault13.gam`: the base file's lines unchanged, ours appended, which
keeps every base index. Map variables (`maps/<stem>.gam`) are not needed
in Pass 2.

### 5.5 Lowering an object script

One program per (script, role), where role is object, critter, map or
spatial. Each program has `start` (first-run initialisation, then a
placeholder result, E2) and only the procedures its blocks need.

- **`OnActivate`** on an object: `use_p_proc` runs the block with
  `IsActionRef X` as `source_obj == X`. The block calls `Activate` to let
  the default happen; when no path through the block reaches `Activate`,
  the procedure calls `script_overrides` (SPEC-005 §2.4) so that the door
  or container does not open. A path that may or may not reach `Activate`
  sets the override on the paths that do not.
- **`OnActivate`** on a critter: talk runs it first; dialogue opens only
  if it reached `Activate`.
- **Locks** (section 9.3) and **terminals** (section 9.1) add their
  procedures to the same program.
- Self-destroying words (`kill_critter`, `destroy_object` on
  `self_obj`) stop the program (SPEC-005 §3.2); the compiler places them
  last in their block and reports any statement after them.

### 5.6 Polling: GameMode and quest scripts

**Critter GameMode** runs inside `critter_p_proc` (E6), after the package
runner, at most once per 10 ticks (`L_LAST_GM`). `GetSecondsPassed`
returns `(game_time - L_LAST_GM) / 10.0`.

**Object GameMode** (activators, containers, doors) runs from
`timed_event_p_proc`, re-armed with `add_timer_event(self_obj, 10, 1)`;
`map_enter_p_proc` arms it. 32 placed instances in the slice: about 32
short runs a game second.

**Quest scripts** run in one manifest global script, `mfquest.int`
(`[[global_scripts]]`, `repeat = 6`, `type = 0`, E26). A step is a frame
(E27), so the dispatcher measures game time itself:

```text
start:                                   ; called every 6 steps
  if not initialised: due[q] = game_time + 3 * ordinal(q)  ; stagger
  for each running quest q in SimId order:
    if game_time >= due[q]:
      due[q] = game_time + 10 * delay(q)   ; delay default 5.0 s (calibration)
      run q's GameMode body
  run pending stage scripts (section 6.2)
  push placeholder
```

`due` lives in program variables (the program lives for the session,
SPEC-024a §4.1, but is not saved; after a load every quest is simply due).
Cost: 18 GameMode quest scripts; a dispatcher run with nothing due is
about 200 instructions; a due body is its statements, tens to a few
hundred instructions; the 1,000,000-instruction host-call cap (E26) is
three orders of magnitude away. A global script has no object (E26), so
every reference it touches goes through section 5.8's guarded slots.

A quest's `MenuMode` blocks are not run.

### 5.7 Trigger volumes

A reference with a primitive on a scripted activator (`mir::Reference::
trigger_volume`) becomes a spatial script record on its map: centre = the
hex under the primitive's centre, radius = the smallest hex radius that
covers the primitive's footprint. `spatial_p_proc` fires on every step
inside (E8), so the program keeps "inside" per mover in `L_FLAGS` for the
player (the slice's triggers all test the player) and runs
`OnTriggerEnter` on the first step in; a 10-tick timer checks the
player's distance and runs `OnTriggerLeave` when the player is outside.
`OnTrigger` runs on every step. Non-player movers are reported.

### 5.8 References at run time

Fallout 2 scripts cannot look an object up by an id. Every reference in
`Module::refs` gets a **slot** k (1-based, `SimId` order) and an exported
variable `r<k>` on its map:

- The map's own map script, `mm<map>.int`, exports `r<k>` for each slot
  whose reference is on that map (in its initial stream, E16).
- **Static** references (doors, containers, activators, markers' objects)
  are resolved by the map script in `map_enter_p_proc` with
  `tile_contains_pid_obj(hex, elevation, pid)` (E18): position and PID
  come from the map stage's placement index. Two static slots with the
  same PID on the same hex and elevation are a build error.
- **Mobile** references (critters) store themselves: their program's
  `map_enter_p_proc` does `r<k> = self_obj`.
- Markers that are not placed objects (`XMarker`, `XMarkerHeading`) need
  no slot: their hex and elevation are compile-time constants.
- A use of slot k is guarded by `cur_map_index == map(k)` and compiled
  with a static fallback when false (the state change goes to globals
  only, section 8), because reading a name another map exported is a VM
  error (E16).

Measured: 97 script-named references stand inside the slice; with
condition, package and group references the slot count stays under 400,
well inside the 1,013-name registry.

### 5.9 Statement lowering

| Source | Engine |
|---|---|
| `SetStage q s` | set stage global (max), stage-done bit, pending bit (section 6.2) |
| `GetStage`, `GetStageDone` | globals (section 6.1) |
| `StartQuest`, `StopQuest`, `GetQuestRunning` | running bit |
| `SetObjectiveDisplayed q n f`, `SetObjectiveCompleted q n f`, `GetObjective*` | objective global 0/1/2 (section 6.3) |
| `Enable`, `Disable`, `GetDisabled` | group global and visibility (section 8) |
| `AddItem`, `RemoveItem`, `GetItemCount`, `RemoveAllItems` | `add_mult_objs_to_inven` with `create_object_sid(pid, 0, 0, -1)`, `rm_mult_objs_from_inven` then `destroy_object`, `obj_is_carrying_obj_pid`, `move_obj_inven_to_obj` (SPEC-005 §2.5); needs the item's PID from the items stage |
| `AddNote`, `GetHasNote`, `RemoveNote` | holodisk global set to 1 / tested / set to 0, plus `display_msg` (section 9.2) |
| `ShowMessage m` | `display_msg(message_str(...))` of the `MESG` text; with buttons, a no-head dialogue whose options set `L_BUTTON` for `GetButtonPressed` |
| `MoveTo r` | same map: `move_to(obj, hex, elev)`; player to another map: `G_ARRIVAL = hex * 4 + elev`, `load_map(map, 0)`, and the destination map script calls `override_map_start` (E25); an NPC to another map: presence (section 7.6) |
| `Kill` | `kill_critter(obj, 0)` |
| `GetDead`, `GetDeadCount` | section 4.8 |
| `EvaluatePackage`, `AddScriptPackage`, `RemoveScriptPackage`, `ResetAI` | runner locals (section 7) |
| `StartCombat t` | `attack_setup(self_obj, t)` |
| `SetAV`, `ModAV`, `RestoreAV`, `GetAV` | hit points: `critter_heal`; Aggression/Confidence: runner flags; others unsupported |
| `RewardXP n` | `give_exp_points(n)` |
| `RewardKarma n` | karma shadow global (engine gap K, section 11) |
| `ShowBarterMenu` | `gdialog_mod_barter(0)` inside dialogue |
| `Lock`, `Unlock`, `GetLocked` | `obj_lock`, `obj_unlock`, `obj_is_locked` |
| `SetOpenState` | `obj_open`, `obj_close` |
| `Activate` | on self: the default action (section 5.5); on another object: `use_obj(r)` |
| `GetLinkedRef` | static, from `XLKR` |
| `GetSelf`, `IsActionRef`, `GetActionRef` | `self_obj`, `source_obj` |
| `GetSecondsPassed`, `GetCurrentTime`, `GetRandomPercent` | section 5.6, 4.8 |
| `PlaceLeveledActorAtMe` | `create_object_sid` of the list's resolved entry (as `mir` resolves leveled lists) |
| `AddToFaction`, `RemoveFromFaction`, `SetFactionRank` | faction-override global; a hostile faction switches team (section 7.5) |
| `DisablePlayerControls`, `EnablePlayerControls` | `game_ui_disable`, `game_ui_enable` |
| `Say`, `SayTo` | `float_msg(obj, text of the topic's first passing INFO for that speaker, 0)` |
| `PlaySound` | dropped until the audio brief |
| `AddPerk`, `AddSpell`, `RemoveSpell`, `AddAchievement`, `IMod`, `RIMod`, `SetCellImageSpace`, `SetEssential`, `SetOwnership`, `ModPCMiscStat`, `PlayIdle`, `SetUnconscious`, `SetPlayerTeammate`, `SetCombatStyle`, `ShowMap`, `MatchRace`, `MatchFaceGeometry` | unsupported, reported |

### 5.10 Code generation and `.int` emission

`transpile::emit::codegen` turns each program into assembler source in
the syntax of `docs/vault-engine-briefs/int-assembler.md` §4: one
`.procedure` per procedure, `.literal` per string, `push.*`, the core
mnemonics, host words by their `extensions::opcode_in` names under
`WordSet::FALLOUT2`. Conventions:

- Procedures take no arguments; values pass through program variables, so
  no call needs the argument protocol beyond `frame`/`drop_frame`.
- Every host-called procedure ends with a pushed placeholder (E2).
- Expressions are evaluated on the stack in source order; every result of
  a host word that pushes is consumed or popped, so the SPEC-017 §8 stack
  model has no finding (E26).
- Conditions short-circuit with `bz`.
- No sfall word and no word outside `FALLOUT2` is emitted unless the
  configuration switches sfall mode on (open question 6).

`emit::int` (in the `emit` crate, behind the writer trait, as AGENTS.md
places local adapters) assembles the text to bytes in the brief's
canonical layout. When `vault-vm` ships the assembler, `emit::int` calls
it instead; the source text is the stable interface.

### 5.11 Hook points used

Pass 2 needs one manifest global script and one hook:

| Row | Purpose |
|---|---|
| `[[global_scripts]] file = "mfquest.int", repeat = 6, type = 0` | quest dispatcher (section 5.6) |
| `[[hooks]] point = "HOOK_GAMEMODECHANGE", file = "mfquest.int", procedure = "mode_changed"` | when dialogue closes (`mod_game_mode` bit `0x4` cleared), run pending stage scripts at once instead of at the next dispatcher run |

No other point is needed: object behaviour lives in object scripts.
`HOOK_USESKILLON` could later give lock and hacking checks to objects
without a script.

### 5.12 Not supported in Pass 2

`MenuMode` and `SayToDone` blocks; item and effect scripts (`OnAdd`,
`OnEquip`, `ScriptEffect*`); followers (`Followers*` quests and the
`IsFollowing*` variables); NPC-to-NPC scenes (`ConvMegaton`, package type
Dialogue between NPCs); perks, spells, actor effects, image spaces,
achievements; cross-object variable access on references with several
placements; `ref` variables holding a reference created at run time;
arrays and string variables (not in Fallout 3 scripts anyway); radio;
combat styles; essential flags; ownership and crime (`MegatonCrimeFaction`
reactions to theft). Each appears in `sim-report.md` with counts.

## 6. Quest state and the Pip-Boy (`transpile::quest`)

### 6.1 Globals per quest

| Global | Content |
|---|---|
| `G_q_STAGE` | highest stage set (`GetStage`) |
| `G_q_DONE_i` | stage-done bits, one global per 31 stages (`MQ01` needs 2) |
| `G_q_FLAGS` | running, completed, failed, start-enabled-applied |
| `G_q_PEND_i` | stage-pending bits (section 6.2) |
| `G_q_OBJ_n` | one per objective: 0 hidden, 1 displayed, 2 completed |
| quest variables | one each (section 5.4) |

### 6.2 Stage scripts

`SetStage q s` at any call site sets `G_q_STAGE` to `max`, the done bit
and the pending bit, at once, so `GetStage` and `GetStageDone` read the
new value immediately. The stage's log-entry scripts run in the
dispatcher (cross-program calls are unreliable, E3): at its next run, or
at once when dialogue closes (section 5.11). For each pending stage, in
ascending stage order, the first log entry whose condition passes runs its
script, applies complete or fail, and starts `next_quest`. Latency: one
dispatcher run, a fraction of a second, or zero after dialogue. A script
that sets a stage and in the same block depends on that stage script's
effects is reported (the slice's INFO scripts set stages in their `end`
block, after which the dialogue closes or the player chooses).

### 6.3 Pip-Boy

The engine's quest list reads `data/quests.txt` (E15). Each Fallout 3
quest with a name and at least one objective becomes a group:

- **Group heading**: the quest's name, written as a new `map.msg` message
  (the table's "location"), so the Pip-Boy lists one heading per quest.
- **Rows**: one row per objective, with description message in
  `quests.msg`, global `G_q_OBJ_n`, display threshold 1, completed
  threshold 2. Displayed objectives show; completed ones strike through.
- Quest-level completion (stage flag "complete quest") marks every
  displayed objective completed, as the original journal greys the quest.
- Stage log text (the journal) is not shown: the engine has no per-quest
  text page (gap J).
- Objective lines wider than 350 pixels in font 101 are reported (they
  run past the list, SPEC-014 §3.41.2).
- Objective targets and map markers are kept in the IR, not lowered.

### 6.4 Rewards

`RewardXP n` is `give_exp_points(n)` (the engine applies its own perks
and level-ups, E20). Karma has no write word (E30): `RewardKarma` adds to
a shadow global `G_KARMA`, which conditions on karma (actor value 23) add
to the engine's value; nothing displays it, and the report names the gap.

## 7. AI package runner (`transpile::packages`)

### 7.1 Where it runs

One shared runner, compiled into every critter program, called from
`critter_p_proc` (E6). It does nothing in combat or dialogue, because the
engine does not call it then. Combat belongs to the engine's AI packets.

### 7.2 Package selection

At most every 300 ticks, and at once when `L_EVP` is set
(`EvaluatePackage`, a package end, map entry):

1. If a script package is set (`AddScriptPackage`), use it.
2. Otherwise walk the actor's package list in order and take the first
   whose schedule holds now and whose condition passes.
3. Schedule: `hour == -1` matches any time; else the package runs from
   `hour` for `duration_hours` (wrapping past midnight); month, day of week
   and date match when -1 or equal (day of week from `game_time / 864000`
   and the calendar of SPEC-018 §6.1).
4. A change of package runs the old package's end script and
   `OnPackageEnd`, then the new one's begin script and `OnPackageStart`.

### 7.3 Five behaviours

| Runner | Source types (slice count) | Behaviour |
|---|---|---|
| Travel | Travel 61, Eat 33, Sleep 36, UseItemAt 1, Find, Patrol 7 (linked chain), Ambush | walk to the location hex; then stand; Patrol cycles the linked references' hexes |
| Sandbox | Sandbox 83, Wander 12 | every 100 to 300 ticks, walk to a random hex from the location's candidate list; otherwise stand, turning now and then |
| Follow | Follow 4, Escort 1, Accompany 2 | keep within 2 hexes of the target: walk when 3 to 7 away, run when further |
| Guard | Guard 3 | stand at the location facing the stored orientation; turn toward the player while within 5 hexes |
| Wait | Dialogue 22, and any type with no usable location | stand where it is, facing its target if any |

Movement is `animate_move_obj_to_tile(self_obj, hex, run ? 1 : 0)`;
progress is checked next turn with `anim_busy` and `tile_num`; three turns
without progress pick the next candidate (E19).

**Locations to hexes, at compile time.** Near a reference on the same
map: that reference's hex. In a cell: the cell's map. Near the editor
location: the actor's placed hex. Near the current location: the hex at
selection. Radius: `ceil(radius / 70)` hexes. For Sandbox, `emit` samples
up to 16 candidate hexes per (package, map) deterministically (sorted by
distance, then hex number, every k-th) from the map stage's walkable mask
within the radius and in the anchor's component, so the runner never
picks an unwalkable hex. A location on another map is handled by presence
(section 7.6).

Eat and Sleep keep their place but lose their furniture action (no sit or
lie animation; gap F).

### 7.4 Per-critter state

Eight locals after the system locals: `L_PKG` (current package ordinal),
`L_PHASE`, `L_GOAL` (target hex), `L_NEXT` (next evaluation tick),
`L_STUCK`, `L_ANCHOR` (editor hex, set on first run), `L_SCRIPTPKG`,
`L_EVP`. All integers, saved with the map.

### 7.5 Combat hand-off and teams

- **Hostility** from the source: an actor is hostile to the player when
  its aggression (`AIDT`) is at least 1 and one of its factions (after
  templates) is an enemy of `PlayerFaction` (`FACT` relations), or its
  aggression is at least 2 and none of its factions is a friend or ally.
  Expected in the slice, to verify on the real data: the 10 raiders
  (their factions and aggression come through their templates), not the
  office protectron (a `PlayerFaction` member), not the townsfolk.
- **Hostile behaviour**: in `critter_p_proc`, when
  `obj_can_see_obj(self_obj, dude_obj)` and the distance is within the
  actor's perception range, `attack_setup(self_obj, dude_obj)` (E23).
- **Teams**: player 0; each group of actors sharing their first
  non-crime faction gets one team, numbered from 100 in faction `SimId`
  order (Megaton townsfolk one team, raiders one team); so attacking one
  townsperson brings in the others, as the engine's team rule does.
  `AddToFaction` into a hostile faction switches team with
  `critter_add_trait(obj, 1, 6, team)`.
- **AI packet**: hostile actors take `map_builder::BuildOptions`'s
  hostile packet, chosen from the base `ai.txt` (lowest `min_hp`, then
  highest `chance`, then lowest number); others keep the existing passive
  choice (contracts 10.1). No new packet is shipped.

### 7.6 Presence across maps

A critter belongs to one map in Fallout 2; Fallout 3 NPCs walk between
the town and their houses by schedule. Pass 2 places a **copy** of a named
actor on every slice map where one of its packages has a location, all
sharing the actor's globals (dead, faction overrides) and each with its
own locals. At map entry and on every package change a copy shows itself
when the selected package's location is on its map and hides otherwise
(`set_obj_visibility`, E17); a copy whose package moves it to another map
while the player watches first walks to the teleport door that leads
there, then hides. A dead actor's other copies stay hidden. Inventories
are not shared between copies (reported). Actors whose packages never
leave their map get no copy.

### 7.7 Cost

Megaton exterior holds about 30 critter scripts, so each gets a turn
every 30 background passes. A turn with nothing due is a time compare
(under 20 instructions); a selection walks at most the actor's package
list (281 slots for 55 bases, 5 on average) with its conditions, a few
hundred instructions every 30 game seconds per critter.

### 7.8 Package data needed

| Field | Used for |
|---|---|
| `PKDT` flags, type, behaviour and type-specific flags (8- and 12-byte forms) | kind, run versus walk, "must reach location" |
| `PLDT` (and `PLD2`) location type, value, radius | location |
| `PSDT` month, day of week, date, hour, duration | schedule |
| `PTDT` target type, target, count or distance | follow target, escort |
| `CTDA` | condition |
| `POBA`, `POEA`, `POCA` with their idle, topic and embedded script | begin, end, change events |

## 8. Enable-parent world state (`transpile::state`)

### 8.1 Groups

Each enable parent with at least one child in the slice is a group: the
parent's own state is the group's state; a child is enabled when the
state is on, or off when its opposite flag is set. A reference that
starts disabled with no parent, and that some script enables, is a group
of one with itself as parent. No chains occur in the slice; a chain in
other data is flattened at build time (a child's group is its root
parent's, with the opposite flags multiplied along the chain).

### 8.2 Where the state lives

One campaign global per group, `G_GRP_k`, 0 or 1, initialised from the
parent's initially-disabled flag. 18 groups in the slice. (Packing into
bits saves globals but costs clarity; the global count is not tight.)

### 8.3 How objects appear and disappear

- The map stage places **every** child of a group, including those that
  start disabled, with the map object's hidden flag (`0x01`, E17) set when
  the child starts disabled. Today the stage skips them; this is a
  map-builder change (section 12, T7).
- The map script applies the state: in `map_enter_p_proc` and
  `map_update_p_proc` (E4, E5) it walks a table of its map's children
  (hex, elevation, PID, group, opposite) and calls
  `set_obj_visibility(tile_contains_pid_obj(hex, elev, pid), hidden)` for
  each (E18). Children carry no script. A 144-child group costs about
  1,200 instructions per update.
- Critter children are handled by their own programs (they move).
- `Enable` or `Disable` of a parent sets the global and, when the group
  has children on the current map, applies them at once in the calling
  program through the same table (the map script's table is duplicated
  into a procedure of the calling program only for groups that program
  touches, to avoid a cross-program call).
- A change made while the group's map is not loaded is applied at the
  next map entry.

### 8.4 Not covered

Children that the player has picked up are not re-hidden (they are no
longer on their hex); children moved by scripts are reported. Navmesh and
blocking: a hidden object blocks nothing, so walls switched on by a group
block only when shown, which is what the source does.

## 9. Terminals, notes, locks (`transpile::terminals`)

### 9.1 Terminals today

A terminal is a scenery object whose program's `use_p_proc` opens a
no-head dialogue (E10, SPEC-006 §3.11 head -1):

- **Reply** = the header text, then the screen's result text after a
  selection.
- **Options** = the menu items whose conditions pass, in order; a
  sub-menu item opens the sub-terminal's node; every node has a converter
  "Back" option and the top node "Exit".
- **Selecting an item**: show its result text, run its result script, add
  its note when "add note" is set (holodisk global, section 9.2), show
  its display note as the next reply's text.
- **Locked terminals** (not flagged unlocked): the first node offers
  "[Science R] Hack" when `has_skill(dude_obj, 12) >= R` with R = 0, 25,
  50, 75, 100 for difficulty 0 to 4, and "[Password]" when the player
  holds the password note. Success sets the terminal's `L_UNLOCKED` and
  gives the plugin's `iXPRewardHackComputer<level>` XP. Difficulty 5 needs
  the password. No minigame and no lockout (open question 3).
- The engine's Science default does nothing (E21), so `use_skill_on_p_proc`
  with Science on a terminal opens the same dialogue.

**Upgrade path.** When the terminal and note modal of
`docs/vault-engine-briefs/terminal-and-note-modal.md` lands, only
`terminals::lower` changes: the IR (`Terminal`, `MenuItem`, `Note`) is
already the modal's model; the menu, sub-menus, password and hacking state
stay the same globals and locals.

### 9.2 Notes today

Each note the slice can give (placed, added by a script, a terminal or a
quest) becomes a Pip-Boy holodisk (E15):

- one `holodisk.txt` row: global `G_NOTE_k`, title message, first text
  message, appended after the base file's rows;
- `pipboy.msg`: title, then the body wrapped to the holodisk text width
  (font 101, from x 255, 370 pixels) one message per line, an
  `**END-PAR**` per paragraph break, and `**END-DISK**`; a body over 499
  lines is cut and reported;
- picking up a placed note sets its global to 1 (`pickup_p_proc` of the
  note's item program) and prints a converter "note added" line with
  `display_msg`; `AddNote` does the same; `RemoveNote` sets 0.
- Voice notes (holotapes) show the text of their topic's responses; image
  and sound notes show their title only, reported.

Measured: 18 placed and 60 script-added notes, so under 80 holodisk rows.
Message ids: title `100000 + k`, text window `200000 + 500 k`.

### 9.3 Locks

Every locked door or container reference gets the lock procedures in its
program, and the map stage sets the locked bit (E22):

- `use_p_proc`: locked and the player carries the key PID → `obj_unlock`,
  a converter "unlocked with key" message, then the default action.
- `use_skill_on_p_proc` with Lockpick (`action_being_used == 9`): when
  `has_skill(dude_obj, 9) >= R` (R from the lock level rounded up to 0,
  25, 50, 75, 100) → `obj_unlock`, `script_overrides`, the plugin's
  pick-lock XP for that level; else a "too hard" message. Level 255: key
  only. No roll and no minigame (open question 3).
- Keys need item PIDs from the items stage; a key that did not convert
  leaves the lock to the skill (reported).

## 10. Layer packaging (`transpile::emit`)

### 10.1 Files

| Path | Content | Base copy |
|---|---|---|
| `scripts/*.int` | every program | — |
| `scripts/scripts.lst` | base rows, then one row per program: `<stem>.int ; <editor id> # local_vars=<n>` | yes (E11: dialogue needs real rows) |
| `text/english/dialog/<stem>.msg` | one per speaker program, terminal program and message-box program | — |
| `data/vault13.gam` | base globals, then ours (`MF_<KIND>_<NAME> := <value>;`) | yes |
| `data/quests.txt`, `data/holodisk.txt` | base rows, then ours | yes |
| `text/english/game/quests.msg`, `pipboy.msg`, `map.msg` | base entries, then ours | yes |
| `maps/*.map` | the map stage's maps with script records attached (section 10.4) | — |
| `vault-mod.toml` | adds `id`, `api = 1`, the `[[global_scripts]]` and `[[hooks]]` rows (5.11) | — |

Program stems: `mm` + 6 base-36 digits (map scripts), `md` (speaker
programs), `mo` (object programs), `mt` (terminals), `mfquest`
(dispatcher). Stems are 8 bytes; numbering is by `SimId` order within
each prefix.

### 10.2 Numbering and namespaces

- Script rows: appended after the base list; row index = base count + i.
- Globals: base count G, then ours in `SimId` order by kind: quest state,
  quest variables, `GLOB`s, groups, notes, once-flags, actor globals.
- `map.msg`, `quests.msg`, `pipboy.msg` ids from 100000 (section 9.2);
  dialogue ids per section 4.10.
- Teams from 100; reference slots from 1.
- **Saves**: a layer that grows `vault13.gam` changes the global count, so
  saves made without it do not load with it, and the other way round (the
  engine compares the counts, `vault_formats::savegame`). Rebuilding with
  more content renumbers globals unless the allocation is locked (open
  question 9).

### 10.3 Determinism and caching

- Every map, list and table is iterated in `SimId` or file order; random
  draws happen only at run time, in the engine.
- No timestamp, host name or path in any file; `--jobs` only parallelises
  per-program code generation, collected in order.
- Stage cache key `sim-1`: the transpiler version, the `[sim]` table of
  `foundry.toml`, the slice, a hash of the MIR inputs the build reads (the
  canonical JSON of every record row it queries), the base files of
  10.1, and the map stage's key and placement index. Program cache: hash
  of the program's lowered IR → `.int` bytes.

### 10.4 Attaching scripts to maps

The map stage writes `map-placements.json` beside `map-report.md` (not in
the layer): for each placed reference, map, hex, elevation, PID, object
id, and whether it is a critter. The `sim` stage reads each built `.map`
with `vault_formats`, adds script records (critter, item, spatial and the
map script, SPEC-003 script categories), sets each object's script id and
the header's map script, and writes it back through `emit::map`. The map
stage itself needs only the changes of section 8.3 and 7.6.

### 10.5 The report: `sim-report.md`

Sections, each with compiled, folded, dropped and fallback counts and the
first owners by editor id:

- `## Summary`: programs, rows, globals, messages, holodisks, quests rows.
- `## Dialogue`: per speaker, INFOs compiled and folded away, topics,
  options, challenges, thresholds; text changes (braces, characters).
- `## Conditions`: per function, count, lowering, fallback value.
- `## Scripts`: per block type, compiled and unsupported; unsupported
  functions by count with owners; hoisted variables; reported orderings
  (section 6.2, 5.5).
- `## Quests`: per quest, stages, objectives, Pip-Boy rows, variables.
- `## Packages`: per type, the runner it maps to; sandbox candidates;
  presence copies.
- `## World state`: groups, children, maps.
- `## Terminals and notes`, `## Locks`.
- `## Engine gaps hit`: which gap of section 11 each loss traces to.

## 11. Engine gaps

What the engine lacks for this layer, most blocking first. Each line is
written so that the brief author can lift it; the briefs themselves are
separate files.

| Id | Need | What it blocks today | Workaround |
|---|---|---|---|
| A | An `.INT` assembler and listing in `vault-vm` (`int-assembler.md`) | every program | local adapter in `emit` |
| T | A terminal and document reader: a modal screen with a header, a menu of selectable lines with sub-menus, result text, and a scrolling page of text (brief written: `terminal-and-note-modal.md`) | the look of 11 terminals and 78 notes | no-head dialogue, holodisks |
| S | Dialogue message files for script rows registered by a manifest's `[[scripts]]` (resolve the dialogue list through the registry as well as `scripts.lst`) | layering without replacing `scripts.lst` | ship a whole `scripts.lst` |
| M | A script word that loads a map at a given hex, elevation and facing under Fallout 2 | `MoveTo` into another map | global plus `override_map_start` on entry |
| P | A way for a critter to leave a map and appear on another (or an off-map critter list) | NPC schedules across cells | presence copies |
| K | A script word that adds to the player's karma (signed) | `RewardKarma` 28 calls | shadow global, no display |
| U | A script word that requests a `map_update_p_proc` broadcast now | world changes from code outside dialogue appear up to 60 s late on objects the program cannot reach | own reference table (section 8.3) |
| J | A per-quest text page in the Pip-Boy (lines added as stages are reached) | the quest journal | objectives only |
| F | Furniture actions for scripts: sit, sleep, lean, with their animations | Eat, Sleep, sandbox furniture | stand |
| G | Dialogue options shown but not selectable (greyed) | New Vegas failed checks | shown with a "25/50" label, selectable to the failure line |
| V | Speech audio and lip sync for converted dialogue (audio brief) | voice | text only |
| R | Reading another script instance's local variable | `GetScriptVariable` on references with several placements | hoisting to globals |
| O | Map or compass markers for quest targets | objective targets | none |

## 12. Work breakdown for Pass 2

AGENTS.md: at most four agents build at once; each owns whole crates;
only the lead edits `docs/contracts.md`, `crates/mojave-convert` and the
root manifests.

| # | Task | Crate | Depends on |
|---|---|---|---|
| T0 | Contracts section 11 from this document: IR, placement index, report | lead | — |
| T1 | Typed `TERM`, `NOTE`, `PACK`, referenced references (appendix A) | `fnv-esm`, `mir` | parsing agent, in progress |
| T2 | `ir` and `build` skeleton: texts, ids, refs, actors, globals; condition lowering (`cond`) with the function table | `transpile` | T1 |
| T3 | `obscript`: lexer, parser, resolution, `functions` table; diagnostics | `transpile` | T2's ids |
| T4 | `emit::int` (assembler adapter, brief vectors A-C), MSG/GAM/LST appenders for the new files, `emit::map::attach_scripts`, `ModLayer` global scripts and hooks | `emit` | — |
| T5 | `dialogue` (4.1-4.10), `quest` (6), `state` (8), `terminals` (9) | `transpile` | T2, T3 |
| T6 | `packages` (7), presence | `transpile` | T2, T7 |
| T7 | map stage: place disabled group children hidden, presence copies, locked bits, `map-placements.json`, hostile packet option | `map-builder` | — |
| T8 | `emit` lowering: `codegen`, tables, numbering, `lower()` | `transpile` | T4, T5 |
| T9 | `sim` stage in `mojave-convert`: cache, report, `all` wiring | lead | T7, T8 |
| T10 | Real-data runs: Super-Duper Mart, then Megaton; engine checks | lead + owner | T9 |

Waves: S1 runs T1 (parsing agent), T2+T3 (one `transpile` agent), T4
(`emit` agent) and T7 (`map-builder` agent) at once. S2: the `transpile`
agent does T5, T6, T8 in order while the lead does T9. S3: T10.

### 12.1 Tests (synthetic only)

- **Front end.** Small plugins built in memory with the `fnv_esm`
  builders (`Topic::new`, `Info::new`, `Quest::new`, `Script::new`, the new
  `TERM`, `NOTE`, `PACK` builders): one town, two NPCs, a topic tree with
  a random group, a say-once INFO, a Fallout 3 challenge with a shared
  failure topic, a New Vegas threshold pair, a quest with three stages and
  two objectives, an enable group with an opposite child, a locked door
  with a key, a terminal with a sub-menu and a password note, a sandbox
  and a travel package. Expected IR as JSON snapshots of synthetic data.
- **Parser.** Hand-written ObScript snippets under `tests/fixtures/`
  (written here, not copied): every grammar rule, case variants,
  comments, malformed input.
- **Codegen.** Every program assembles; `vault_vm::IntProgram::parse_in`
  accepts it under `WordSet::FALLOUT2` and `vault_vm::stack_check` finds
  nothing (a dev-dependency on `vault-vm` at the pinned revision; open
  question 10).
- **Behaviour.** Programs run in `vault-vm` with a test host that
  implements the words the dialogue and quest code use (globals, locals,
  `gsay_*` recording, `random` scripted): picks, random groups, say-once,
  the challenge both ways, stage pending, objective globals.
- **Tables.** `quests.txt`, `holodisk.txt`, `vault13.gam` and the MSG
  files round-trip through `vault_formats` parsers; numbering is stable
  under reordering of input records.
- **Determinism.** Two builds and `--jobs 1` versus `--jobs 4` give
  identical layers.

### 12.2 Acceptance: "Super-Duper Mart playable"

On the real layer, in the engine, by the owner (headless checks first):

1. Every `.int` loads: the engine log has no mod-script refusal for the
   layer (E26); `sim-report.md` lists no build error.
2. Entering `SuperDuperMart`: the 10 raiders stand or sandbox; seeing the
   player, they start combat and fight with an engine packet; killing them
   updates their kill counters.
3. The 4 locked references open with their key or with Lockpick at the
   required level, and refuse otherwise.
4. The 2 terminals open as a menu; a sub-menu and Back work; a locked one
   offers the Science check.
5. The group in the cell switches when its quest stage is set.
6. Leaving and re-entering (saved map) keeps opened locks, dead raiders
   and the group state; a save and load keeps them too.

### 12.3 Acceptance: "Megaton"

1. Talking to `LucasSimms` opens his greeting, the hub and his `MS11`
   topics; choices follow; goodbye ends; repeated talk respects say-once.
2. The door to `MegatonMoriartysSaloon` leads into the saloon map and back
   (needs the interior as a real map in the layer, map-builder work).
3. A Fallout 3 challenge works both ways: `ColinMoriarty`'s
   `MegMoriartyChoice2a` (difficulty 2, sets `MQ01` stage 40) shows a
   chance, a pass says the challenge INFO and sets the stage, a fail says
   a `SpeechChallengeFailure` line; the outcome is kept on re-asking.
4. One quest advances: an `MS11` stage set in Simms' dialogue displays its
   objective in the Pip-Boy under the quest's heading, and completing it
   strikes it through.
5. A note added by dialogue or picked up reads in the Pip-Boy.
6. NPCs with schedules are where their package puts them at a chosen hour
   (advance time with rest) and walk between places.

## 13. Owner decisions (locked 2026-11-23)

The owner accepted all 15 defaults below as written, on 2026-11-23. They
are decisions for Pass 2, no longer open: forever (1), 1:1 (2), automatic
lock and hack (3), holodisks with voice notes as text (4), presence copies
for named actors (5), sfall off (6), a whole `scripts.lst` with brief S
filed (7), objectives only (8), `sim-ids.toml` off for Pass 2 (9),
`vault-vm` as a pinned dev-dependency for tests (10), Megaton and raider
teams (11), essential ignored and reported (12), a shadow karma global
(13), hello barks off (14), New Vegas failed checks shown as "25/50" and
selectable (15). The questions are kept as they were asked.

1. **Speech challenge memory.** Remember a challenge's outcome forever
   (re-asking repeats the branch), or roll on every selection? Default:
   forever, which removes save-free re-rolling and matches how failed
   challenges usually end the topic.
2. **Skill scale.** Compare Fallout 3 and New Vegas thresholds to Fallout
   2 skill percentages 1:1? Default: 1:1, and the percent formula of
   section 4.7 with the plugin's own settings.
3. **Hacking and lock picking.** No minigame: automatic success at the
   required skill, XP from the plugin's settings. Or a `roll_vs_skill`
   with a lockout after failures? Default: automatic.
4. **Notes.** Holodisks in the Pip-Boy, with a message-line notice?
   Default: yes; voice notes as text.
5. **NPCs across maps.** Presence copies for actors whose packages leave
   their map, or pin every NPC to the cell it stands in? Default: copies,
   named actors only.
6. **sfall mode.** Keep it off (no `message_box`, plain Fallout 2 words)?
   Default: off.
7. **`scripts.lst`.** Ship a whole copy (needed for dialogue files, E11),
   with a brief for gap S? Default: whole copy now, brief S filed.
8. **Quest journal.** Show only objectives, or also one holodisk per
   quest with its log entries as reached? Default: objectives only.
9. **Stable numbering.** Keep an allocation file (`sim-ids.toml`, an
   input of the build) so that globals and rows keep their numbers across
   rebuilds and saves survive? Default: off for Pass 2, on before long
   playtests.
10. **`vault-vm` in tests.** Allow it as a dev-dependency at the pinned
    revision (one-way, tests only)? Default: yes.
11. **Teams.** Megaton townsfolk as one team (attack one, all respond),
    raiders as one team? Default: yes.
12. **Essential NPCs.** The source protects some NPCs from death; the
    engine has no such flag. Default: ignore and report.
13. **Karma.** Shadow global with no display until gap K? Default: yes.
14. **Hello barks.** `float_msg` greetings when the player passes?
    Default: off in Pass 2.
15. **New Vegas failed checks.** Show them with a "25/50" label and let
    them be chosen (the source's failure line)? Default: yes.

## Appendix A. Needed from the parsing agent

Checked against the parsing agent's uncommitted work on 2026-11-23
(`fnv_esm::records::{terminal, note, package}`, `mir::logic`). Already
there, and used as is: `TerminalRow` with items (text, result text,
flags, display note, sub-terminal, script source, conditions), `NoteRow`
(type, text, topic, sound, texture, quests), `PackageRow` (type,
schedule, two locations, two targets, dialogue topic, conditions, event
scripts), `Records::conditions(owner)`, `actor_packages`,
`package_actors`, `quest_targets`, `script_references`, `ref_referrals`
with `RefReason` (quest target, script, condition, linked ref, enable
parent and child, package target and location), `referenced_refs`,
`Records::game()`, and the 3-byte `INFO` `DATA` and 16/20-byte `TRDT`
forms. Still needed for T1:

1. **`PKDT` flags beyond the general word**: the behaviour flags and the
   type-specific flags now kept as `PackageData::rest` (run versus walk,
   must reach location, and the sandbox and travel options), decoded for
   both the 8- and 12-byte forms; and the event sections' idle and topic,
   not only their script source.
2. **`INFO`**: `KNAM` (the label actor value or perk of a check) and named
   flag accessors for random-end, run-immediately, info refusal, speech
   challenge (`0x80` of the first flag byte), say-once-a-day and
   always-darken (second byte); `DNAM` kept as stored.
3. **Small records**, typed and tabled: `GLOB` (`FNAM` type, `FLTV`
   value), `GMST` (editor id and value; the build reads the
   speech-challenge and XP-reward settings), `MESG` (title, text, flags,
   buttons with their conditions), `FACT` (flags and the `XNAM`
   relations: faction, modifier, combat reaction), `NPC_` and `CREA`
   `AIDT` (aggression, confidence, energy, responsibility, mood,
   services, assistance) and `VTCK` voice type, following templates as
   the actor documents do.
4. **Referenced references, two more reasons**: condition parameters that
   name a reference (`GetIsReference`, `GetDistance`, `GetInSameCell`,
   `GetDead` and `GetDisabled` with a reference parameter), and teleport
   doors whose destination is a referenced reference, so that `MoveTo`
   into another map can find the door's map.
5. **Embedded script contexts**: every body (`INFO` begin and end, stage
   log entry, terminal item, package event) reachable with its `SCRO`,
   `SCRV` and `SLSD`/`SCVR` lists, not only its source, so that the
   compiler resolves ambiguous names through the script's own reference
   list.
6. **Placement facts the map stage needs** (for T7, not the parsing
   agent, listed here for completeness): `XPRM` bounds on trigger volumes
   (present), lock level and key (present), initially-disabled and enable
   parent (present), `XLKR` chains (present).

## Appendix B. How the counts were measured

- `out/bin/mojave-convert-int probe esm` on `Fallout3.esm`: record and
  subrecord counts, undecoded subrecords (copied to
  `out/sim-spec-scratch/probe-fo3.txt`).
- A read-only Python pass (`python3 -I`, standard library only) that
  walks the plugin's groups, decompresses records and keeps the
  subrecords of the record types above in a scratch pickle outside the
  repository; slice membership by cell group and by position for
  persistent references; speaker detection by `GetIsID == 1` conditions;
  function names from the public condition-function list; script figures
  from a line tokenizer (block heads, declarations, and identifiers that
  are neither variables, keywords nor editor ids). Counts of candidate
  functions are approximate by a few names (variables of other scripts
  can be miscounted as functions); block, record and condition counts are
  exact.
- The same walk over `FalloutNV.esm` for the New Vegas check comparison.
