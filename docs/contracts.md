# Shared contracts

Interfaces that more than one crate relies on. The lead owns this file.
Change it first, then the code.

## 1. Source space

The source game is Fallout 3 or Fallout: New Vegas (`fnv_esm::GameVariant`,
re-exported as `mir::GameVariant` and `mojave_convert::GameVariant`). Both
use the space, the cell grid and the plugin layout below. `mojave-convert`
decides the game once, before extract (`mojave_convert::source::resolve`:
the command line, then the slice, then the install's master and its `HEDR`
version; a disagreement is an error), and passes it to
`mir::extract_for_game`; every later stage reads it from `Inputs::game` or
`Mir::game()`. A stage whose output depends on the game hashes
`GameVariant::id` into its cache key from Fallout 3 on and nothing for New
Vegas, so New Vegas keys made before both games stay valid.

Positions from the source game are kept as they are stored:

- right-handed, **X east, Y north, Z up**, in game units;
- an exterior cell is 4096 units square; cell `(cx, cy)` covers
  `x ∈ [4096·cx, 4096·(cx+1))`, `y ∈ [4096·cy, 4096·(cy+1))`;
- a cell's land grid is 33 × 33 vertices at 128-unit spacing, row 0 at the
  south edge, column 0 at the west edge (verified on real data: 99.9 % of
  neighbouring cells share their edge exactly under this reading, about
  50 % under either flip);
- land texture quadrants are 0 south-west, 1 south-east, 2 north-west,
  3 north-east; an opacity position is `row * 17 + column` within its
  quadrant, row 0 south, column 0 west (verified the same way);
- land normals are signed bytes of length about 127;
- mesh node rotations are 3 × 3 matrices whose rows act on column vectors,
  child first then parent (verified on real skinned meshes);
- rotations on placed references are Euler angles in radians, stored
  `(rx, ry, rz)`, positive clockwise looking down the axis. A point is
  transformed as `R = Rz(-rz) · Ry(-ry) · Rx(-rx)` applied to the scaled
  local position, then translated. Only `mir::Transform::matrix` implements
  this; nothing else rebuilds it. **Provisional:** the sign is settled for
  rotation about Z, which is what almost every static uses. The order of
  the three axes is a calibration item for the first real-data run.

Nothing is rescaled or re-axed at extraction time, so calibration never
needs a second extract.

## 2. Content hashes and the MIR directory

- A content hash is the lowercase hex SHA-256 of the **source file bytes**
  (the NIF as stored, the DDS as stored). It names the converted file.
- Archive paths are compared lowercase with `\` separators, as stored.

```
<mir>/
  records.sqlite                 typed core tables + lossless raw subrecords
  meshes/<hash>.glb              one per distinct source mesh
  textures/<hash>.png            one per distinct source texture, RGBA8
  cells/<worldspace>/<x>_<y>.json    exterior cells, editor id lowercase
  interiors/<editor_id>.json         interior cells, editor id lowercase
  report.md
  manifest.json                  path → hash index, the extract inputs and the game (`game`)
```

Only `crates/mir` reads or writes this directory. Other crates go through
its API, never through the files.

**Manifest fields** (2026-11-23). Besides the path index and the extract
inputs, `manifest.json` records `game` (the plugin list's game, detected
from its first plugin or given to `mir::extract_for_game`; a manifest
without it reads as New Vegas), `records_revision` (the revision of the
tables in `records.sqlite`, `mir::RECORDS_REVISION`, within one
`SCHEMA_VERSION`: extract rebuilds an older one, and a reader of a table an
older file lacks returns `MirError::MissingTable`; 0 when absent) and
`notes` (things worth knowing that are not issues: a persistent reference
in a grid cell with no cell record, a cell whose land comes from the parent
worldspace).

**Tables** of `records.sqlite` (revision 1): `records`, `worldspaces`,
`cells`, `placed_refs`, `lands`, `navmeshes`, `statics`, `doors`,
`containers`, `inventory`, `misc_items`, `weapons` (with the combat
columns), `armors`, `ingestibles`, `ammo`, `npcs`, `creatures`,
`land_textures`, `texture_sets`, `scripts`, `script_variables`,
`embedded_scripts`, `quests`, `quest_stages`, `quest_objectives`, `topics`,
`topic_quests`, `infos`, `info_responses`, `conditions`, `terminals`,
`terminal_items`, `notes`, `note_quests`, `packages`, `actor_packages`,
`script_refs`, `quest_targets`, `bad_records`, plus `meta` (the game,
`meta.game`), `plugins` and `raw_subrecords` (the lossless store). Revision
1 added the game, terminals, notes, packages, actor packages, the script
and quest-target references and the weapon combat columns.

**Versions (2026-11-24).** `mir::SCHEMA_VERSION` covers the directory layout,
the JSON documents and the SQLite schema, and is 3: version 3 added
`Reference::primitive` to the cell documents (below). A directory of another
version is refused by `Mir::open` (`MirError::SchemaVersion`) and rebuilt by
`extract`, which is how an older cache of cells is replaced. The records
revision (`mir::RECORDS_REVISION`, 2) covers only the tables of
`records.sqlite` within one schema version.

**Primitive volumes (schema 3).** A reference's `primitive` is
`{ bounds: [x, y, z], shape }`, left out of the JSON when the reference has
none or a bound is not a finite number. It comes from the `XPRM` subrecord
(32 bytes in every reference of both retail masters: bounds, an editor
colour, an unnamed float and the shape code) and is kept for every reference
that has one, not only triggers: room, collision and multibound markers,
acoustic spaces and portals carry them too. `shape` is `"box"` (code 1),
`"sphere"` (2), `"portal_box"` (3) or `{"other": code}` (any other code;
`mir::PrimitiveShape`). `bounds` are the extents along the reference's local
axes, as stored, before the reference's scale; public documentation reads them
as half-extents (a box reaches from `-bounds` to `+bounds`, and a sphere's
first is its radius), and the converter uses that reading. The colour and the
unnamed float are not kept. `trigger_volume` is unchanged: a primitive on an
`ACTI` or `TACT` base. Counts in the retail masters (2026-11-24, from
`cargo run -p fnv-esm --example triggers`): Fallout 3, 4,137 references with a
primitive (1,111 on activators: 1,101 boxes and 10 spheres, 17 of them with a
stand-in model), no `XPRM` of another size; New Vegas, 5,677 (1,266 on
activators: 1,261 boxes and 5 spheres, 66 with a stand-in model), none of
another size.

**Tables, revision 2** (`docs/simulation-layer-spec.md`, appendix A; the
queries are in `crates/mir/src/simlogic.rs` and `logic.rs`). A reader of any
of them on a file written before revision 2 returns `MirError::MissingTable`.
New tables and what reads them (all methods are on `mir::Records`):

| Table | Columns | Typed query |
|---|---|---|
| `globals` | `form`, `editor_id`, `type_code`, `kind`, `value` | `globals()`, `global(&FormKey)`, `global_by_name(&str)` return `GlobalRow { form, editor_id, kind: Option<GlobalType>, value: Option<f32> }` (the initial value, whatever the type) |
| `game_settings` | `form`, `editor_id`, `kind`, `int_value`, `float_value`, `text_value` | `game_settings()`, `game_setting(&str)`, `game_setting_f32(&str)`, `game_setting_i64(&str)`; `GameSettingRow { form, name, value: Option<GameSettingValue> }`, typed by the name's first letter |
| `messages` | `form`, `editor_id`, `title`, `text`, `icon`, `flags`, `display_time`, `buttons` | `messages()`, `message(&FormKey)` return `MessageRow { form, editor_id, title, text, icon, flags, display_time, buttons }` with `is_message_box()` and `is_auto_display()` |
| `message_buttons` | `message`, `seq`, `text`, `conditions` | the `buttons` of a `MessageRow`: `MessageButtonRow { seq, text, conditions: Vec<ConditionRow> }` |
| `factions` | `form`, `editor_id`, `name`, `flags`, `flags2`, `relations` | `factions()`, `faction(&FormKey)`, `faction_by_name(&str)` return `FactionRow { form, editor_id, name, flags, relations }` with `reaction_towards(&FormKey)`, `is_hidden_from_pc()`, `is_evil()`, `tracks_crime()` |
| `faction_relations` | `faction`, `seq`, `other`, `modifier`, `reaction` | the `relations` of a `FactionRow`: `FactionRelationRow { other, modifier, combat_reaction }` with `reaction()` |
| `actor_ai` | `actor`, `kind`, `aggression`, `confidence`, `energy_level`, `responsibility`, `mood`, `services`, `trained_skill`, `trainer_level`, `assistance`, `aggro_radius_behaviour`, `aggro_radius`, `ai_from`, `voice`, `voice_from` | `actor_ais()`, `actor_ai(&FormKey)` return `ActorAiRow { actor, kind ("npc" or "creature"), ai: Option<AiData>, ai_from, voice, voice_from }`, resolved through the template flags (AI data flag `0x0010`, traits flag `0x0001`); `ai_from` and `voice_from` name the actor read |
| `actor_factions` | `actor`, `seq`, `faction`, `rank`, `from_actor` | `actor_factions(&FormKey)` returns `[ActorFactionRow { faction: Option<FormKey>, rank, from }]`, through the factions flag (`0x0004`) |
| `info_links` | `info`, `kind`, `seq`, `topic` | read into `InfoRow`: kind `choice` (`choices`), `link_from` (`link_from`), `follow_up` (`follow_up`), `add_topic` (`add_topics`); `info(&FormKey)` and `topic_info_rows(&FormKey)` return `InfoRow`, with `data()` for the flag accessors |
| `package_events` | `package`, `event`, `idle`, `topic`, `script_source` | the `events` of a `PackageRow` (`logic.rs`): `PackageEventRow { event ("on begin", "on end" or "on change"), idle, topic, script_source }` |
| `script_contexts` | `owner`, `context`, `script_type`, `flags`, `ref_count`, `compiled_size`, `variable_count`, `has_source`, `has_compiled` | `script_contexts(&FormKey)`, `script_context(&FormKey, &str)` return `ScriptContextRow` (those columns, `source`, `references`, `locals`) with `local(&str)` and `form_at(u32)` |
| `script_ref_list` | `owner`, `context`, `seq`, `kind`, `form`, `raw` | the `references` of a `ScriptContextRow`: `ScriptRefRow { index (1-based, as compiled code names it), resolved, entry: ScriptRefKind::Form(raw id) or Local(index) }` |
| `script_locals` | `owner`, `context`, `idx`, `name`, `flags` | the `locals` of a `ScriptContextRow`: `ScriptLocalRow { index, name, flags }` with `is_integer()` (flag bit 0) |
| `decode_skips` | `area`, `reason`, `count` | `decode_skips()` returns `[DecodeSkipRow { area ("PACK/PKDT"), reason, count }]`: what the decoders could not use, over the whole plugin list |

Columns revision 2 added to older tables: `infos.flag_bytes`, `knam`,
`knam_sig` and `speech_challenge` (`InfoRow` fields of the same names);
`packages.pkdt_size`, `behaviour_flags`, `behaviour_raw` and `type_flags`
(`PackageRow::pkdt_size`, `behaviour_flags`, `behaviour_flags_raw`,
`type_flags`; the raw word keeps the uninitialised fill and the plain one is
`None` when `PKDT` stores it) and `conditions.param1_ref`, `param2_ref`
(`ConditionRow::param1_ref`, `param2_ref`: the placed reference a parameter
names, for the functions in `fnv_esm::REFERENCE_PARAMETER_FUNCTIONS`).

`Records::ref_referrals` (and `referenced_ref_reasons`, grouped by reference)
gives each reference something points at with a `RefReason`: `QuestTarget`,
`Script`, `Condition`, `LinkedRef`, `EnableParent`, `EnableChild`,
`PackageTarget`, `PackageLocation` and, new in revision 2, two more: `RefReason::ConditionParam` (a condition's parameter
names the reference) and `RefReason::TeleportDestination` (the destination
door of a teleport door). `referenced_refs()` keeps the set it always
returned: only the reasons for which `RefReason::is_legacy()` is true, so
declutter output is unchanged. `referenced_refs_all()` adds the two new
reasons, and is what a consumer that wants every reference something points
at calls.

Row counts of the retail masters are in `docs/handoff-pass-1.md`, section 13.

**Paths.** `mir` owns path normalisation, and nothing else does it:
lowercase ASCII, `/` to `\`, a leading `data\` dropped, and the root
prefix added when missing (`meshes\` for a record's model path,
`textures\` for a texture-set or mesh texture path). Archive lookups
compare normalised strings. Bytes above 0x7F are decoded as Windows-1252
on the plugin side and Latin-1 on the archive side; the two differ only in
0x80 to 0x9F, which no shipped path uses. A lookup that fails for that
reason is reported as a missing file, not an error.

**Form identity.** A form is written `"<plugin, lowercase>:<local id, 6 hex
digits>"`, for example `"falloutnv.esm:0daebb"`, resolved through the
owning plugin's master list. Raw 32-bit form ids never leave `fnv-esm`
unresolved.

**Lookup order.** A loose file under the data directory wins over any
archive. Archives are searched in a fixed, sorted order, with `Update.bsa`
first. The order is recorded in `manifest.json`.

**Numbers.** JSON carries no non-finite float. A non-finite value in a
source record is an extraction error for that record: it is reported and
the record is skipped.

**Bad records.** One undecodable record never stops an extract. It is
counted, listed in `report.md` with its form and offset, and skipped.

**Caches below the stage keys (2026-11-24).** A stage key (`cache/map.key`)
says whether a whole stage can be skipped. When it cannot, the map stage
takes units from a content-hash store under `--cache-dir` (default
`<out>/cache`): every renderer call the slicer makes, keyed by the scene's
content, the texture content hashes, the window, the calibration, the backend
and `foundry_render::OUTPUT_REVISION`; every terrain ground, keyed by its land
inputs, the sampler digest (`GroundSampler::digest`), the overlay pixels, the
options (except `jobs`) and `map_terrain::OUTPUT_REVISION`; and every
preview PNG, keyed by its pixels and palette. No key holds a path or a time.
An entry is a header (magic, kind, key, length, SHA-256 of the payload)
followed by the payload; an entry failing any check is deleted and its work
redone; entries are written under a temporary name and renamed; nothing is
evicted. Source: `crates/mojave-convert/src/cache.rs`.

## 3. Mesh interchange (`.glb`)

`.glb` is the interface between extraction and rendering. Files are valid
glTF 2.0 binary. The writer and the reader both live in `nif::glb` and are
tested as a round trip; `foundry-render` reads meshes through that reader.

- glTF is Y-up. A source point `(x, y, z)` is written as `(x, z, -y)`;
  normals likewise. Readers invert with `(x, -z, y)`.
- **Units stay game units.** `asset.extras` carries
  `{"mojave": {"units": "game", "source_up": "z"}}` so that nobody mistakes
  them for metres.
- Node transforms are baked into vertex data. A file has one scene, one
  root node, and one mesh whose primitives are the source shapes in file
  order.
- Each primitive has `POSITION`, and `NORMAL`, `TEXCOORD_0`, `COLOR_0`
  (RGBA float) when the source has them, with `u32` indices, mode
  triangles. Strips are expanded.
- Each primitive has a material. `baseColorTexture` is omitted; the texture
  is named instead in `material.extras.mojave.textures`, the list of source
  texture paths (lowercase, `\` separators, slot order: diffuse, normal,
  glow, ...), because textures live beside the meshes by hash, not inside
  them. `alphaMode` is `OPAQUE`, `MASK` (with `alphaCutoff`) or `BLEND`.
- Collision is metadata only: `nodes[0].extras.mojave.collision`.
- Output is deterministic: fixed key order, no generator timestamp.

## 4. Tile space and the projection

Fallout 2 screen geometry comes from `vault_formats::geometry::HexGrid`. A
map has 100 × 100 floor squares and 200 × 200 hexes on the same plane.

**Tile space** is continuous `(s, t, e)`: `s` and `t` in floor squares, `e`
in storeys (one storey is the roof offset). With the grid origin at zero:

```
screen_x =  48·s + 32·t
screen_y = -12·s + 24·t - 96·e
```

These are the columns `(+48, -12)`, `(+32, +24)`, `(0, -96)` of the 2 × 3
projection matrix. `foundry-render` must not hard-code them: it derives
them from `HexGrid::square_corner` differences, and a test pins the values.

Relation to the integer grids, read from `HexGrid`:

- square index `q`: `s = 99 - q % 100`, `t = q / 100`; its corner is at
  screen `(-16, -2)` plus the projection of `(s, t, 0)`;
- hex index `x`: with `h = 199 - x % 200`, `r = x / 200`, the hex reference
  is the projection of `(h/2, r/2 + (h mod 2)/4, 0)` in exact arithmetic,
  and its centre is `(+16, +8)` from the reference;
- to find the hex or square under a point, project to the screen and call
  `HexGrid::pick_hex` / `pick_square`. Do not re-derive picking.

**View direction.** The projection's null space is `d = (2, -3, -1)` in
`(s, t, e)`. Depth is the coordinate along `d`, measured in metric space
(scale the components by tile and storey size first); larger is farther.
The viewer sees faces whose outward normal has a negative component along
`d`. Depth sorting and the depth buffer both use this one definition.

**Source to tile space.** A map window is anchored at a source point
`(x_west, y_north)`:

```
s = (x - x_west) / tile_units          east  → +s
t = (y_north - y) / tile_units         south → +t
e = (z - ground_z) / storey_units
```

`ground_z` is the terrain height under the object, so the flattened map
keeps objects on the floor. Defaults, all overridable in `foundry.toml`:

| Key | Default | Meaning |
|---|---|---|
| `units_per_metre` | 70.0 | source units per metre |
| `tile_metres` | 2.0 | one floor square |
| `pixels_per_metre_vertical` | 32.0 | so one storey, 96 px, is 3 m |
| `yaw_quarter_turns` | 0 | rotates the window about Z in 90° steps |

So `tile_units = 140`, `storey_units = 210`, and one map covers 14 000
source units on a side, about 3.4 cells.

## 5. Images

Crates exchange images as plain buffers, not through a shared image type:

- RGBA8, row-major, top row first, straight (non-premultiplied) alpha,
  `width * height * 4` bytes;
- indexed, row-major, one byte per pixel, index 0 transparent;
- depth, row-major `f32`, the depth of section 4, `f32::INFINITY` where
  nothing was drawn.

Colour values in MIR textures are the stored bytes; no gamma conversion is
applied at extraction.

## 6. Determinism

- Identical inputs and configuration give byte-identical outputs.
- No hash-order iteration reaches an output. No timestamps, host names or
  absolute paths in outputs.
- `--jobs` changes speed only. Parallel work is collected and written in a
  sorted order.
- A cached unit is byte-identical to the work it replaces. Bump the crate's
  `OUTPUT_REVISION` (`foundry_render`, `map_terrain`) when its output
  changes, and the `cache` module's revision constants (`RENDER_REVISION`,
  `GROUND_REVISION`, `PREVIEW_REVISION`) when a key or an encoding changes.
- The GPU and CPU render paths agree within the tolerance stated in
  `foundry-render`. Byte-identical output across machines is guaranteed
  only for the CPU path; a cache entry records which path produced it.

## 7. Errors

Each crate has one `thiserror` error enum. Parse errors carry the byte
offset. No `unwrap` or `expect` on data that came from a file.

## 8. Sprite foundry interfaces (M2)

The four M2 crates do not depend on each other. `mojave-convert` chains
them, passing the plain buffers of section 5.

### 8.1 `foundry-render`

```rust
pub struct Calibration { /* section 4 keys, plus supersample (4), alpha_threshold (0.5),
                           contrast (1.0 = off), key light direction and intensity, ambient */ }
impl Calibration { pub fn from_toml(text: &str) -> Result<Self, RenderError>; }

pub struct Projection { /* built from a Calibration and HexGrid */ }
impl Projection {
    pub fn tile_from_source(&self, anchor: [f32; 3], p: [f32; 3]) -> [f32; 3]; // (s, t, e)
    pub fn screen(&self, ste: [f32; 3]) -> [f32; 2];
    pub fn depth(&self, ste: [f32; 3]) -> f32;
    pub fn unproject(&self, screen: [f32; 2], depth: f32) -> [f32; 3];          // inverse of the two above
}

pub struct Instance<'a> {
    pub mesh: &'a nif::StaticMesh,
    pub transform: [[f32; 4]; 3],            // source space, from mir::Transform::matrix
    pub textures: &'a dyn TextureSource,     // texture path -> RGBA8 image, None if missing
}
pub struct Scene<'a> { pub anchor: [f32; 3], pub instances: Vec<Instance<'a>> }

pub enum Window { Fit, Rect { x: i32, y: i32, width: u32, height: u32 } } // screen pixels, anchor at (0, 0)

pub struct RenderOutput {
    pub origin: [i32; 2],    // screen position of pixel (0, 0), relative to the projected anchor
    pub width: u32, pub height: u32,
    pub rgba: Vec<u8>,       // alpha is 0 or 255 after the threshold
    pub depth: Vec<f32>,
}
pub trait Renderer { fn render(&mut self, scene: &Scene, window: Window) -> Result<RenderOutput, RenderError>; }
pub struct CpuRenderer;      // reference path, deterministic everywhere
pub struct GpuRenderer;      // wgpu, headless; construction fails cleanly with no adapter
pub fn renderer(backend: Backend, calibration: &Calibration) -> Result<Box<dyn Renderer>, RenderError>; // Auto | Cpu | Gpu
```

`Window::Rect` lets a caller render a large scene in pieces. A missing
texture renders as flat mid-grey and is counted, not an error.

### 8.2 `quantize`

```rust
pub struct PaletteMap { /* allowed indices and a lookup table */ }
impl PaletteMap { pub fn new(palette: &vault_formats::Palette) -> Self; }
pub enum Dither { None, Ordered { strength: f32 } }
pub fn quantize(rgba: &[u8], width: u32, height: u32, map: &PaletteMap, dither: Dither) -> Vec<u8>;
pub fn outline(indexed: &mut [u8], width: u32, height: u32, map: &PaletteMap); // 1 px dark inner edge
pub fn to_rgba(indexed: &[u8], palette: &vault_formats::Palette) -> Vec<u8>;
```

Index 0 is produced only for alpha below 128. The colour-cycling ranges
(`palette::CYCLE_RANGES`), the red pulse index and unmapped indices are
never produced for opaque pixels.

### 8.3 `emit`

```rust
pub struct FrmSpec { pub fps: u16, pub action_frame: u16, pub directions: Vec<FrmDirection> } // 1 or 6
pub struct FrmDirection { pub shift: [i16; 2], pub frames: Vec<FrmFrameSpec> }
pub struct FrmFrameSpec { pub width: u16, pub height: u16, pub displacement: [i16; 2], pub pixels: Vec<u8> }

pub trait FormatWriters {            // the swap point for upstream writers
    fn frm(&self, spec: &FrmSpec) -> Result<Vec<u8>, EmitError>;
    fn lst(&self, lines: &[LstLine]) -> Result<Vec<u8>, EmitError>;
    fn msg(&self, entries: &[MsgEntry]) -> Result<Vec<u8>, EmitError>;
    fn gam(&self, section: GamSection, vars: &[GamVar]) -> Result<Vec<u8>, EmitError>;
    fn pro(&self, prototype: &Prototype) -> Result<Vec<u8>, EmitError>;   // wraps vault-formats
    fn map(&self, map: &MapFile) -> Result<Vec<u8>, EmitError>;           // wraps vault-formats
}
pub struct LocalWriters;             // this repository's adapters
pub struct ModLayer { /* manifest + BTreeMap<vfs path, bytes> */ }
impl ModLayer { pub fn write(&self, dir: &Path) -> Result<(), EmitError>; }
```

A sprite frame is placed by the engine with its bottom-centre at the hex
anchor plus the direction shift (`FrmFrame::world_rect`). `emit` provides
the helper that turns a `RenderOutput` origin and size into that shift.

### 8.4 `foundry-gallery`

```rust
pub struct Tile<'a> { pub label: &'a str, pub metrics: &'a [(String, String)],
                      pub rgba: &'a [u8], pub width: u32, pub height: u32,
                      pub compare: Option<(&'a [u8], u32, u32)> }
pub fn contact_sheet(tiles: &[Tile], options: &SheetOptions) -> Sheet;   // RGBA8
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8>;
```

### 8.5 As built

Where the M2 crates differ from the sketches above, the code wins:

- every function that takes a buffer returns `Result` (bad lengths are an
  error, never a panic); `sprite_shift` too, since a shift can overflow;
- renderers are built with `::new(&Calibration)`; `TextureSource` is
  `fn texture(&self, path: &str) -> Option<Cow<'_, Texture>>`;
- the projected anchor of a sprite is the **hex centre**
  (`HexGrid::hex_center`);
- the key light direction is given in map axes `(s, t, e)`, so it stays
  upper-left whatever the yaw; yaw turns source offsets counter-clockwise
  seen from above;
- depth is the metric position projected on the unit view direction, in
  metres; a resolved pixel's depth is its nearest covered sub-sample;
- alpha 128 or more is opaque everywhere; palette white previews as 252
  because the palette's 6-bit channels are multiplied by 4;
- the pipeline's default render path is the CPU one, for byte-identical
  output on every machine; `--backend gpu` opts in.

## 9. Map pipeline interfaces (M3)

Three crates, independent of each other; `mojave-convert` chains them.
Shared, already built: `mir::MapWindow` (the square of the worldspace one
map covers; its north-west corner is the tile-space origin),
`mir::GroundSampler` (terrain height, normal, slope), and
`foundry_render::Projection`.

**Map screen space** is `HexGrid::default()`: origin zero. The engine
draws the floor tile of square `q` with its top-left at
`square_corner(q, false)`, and an object at hex `x` with its frame's
bottom-centre at `hex_center(x)` plus the FRM shift plus the object's own
displacement. `Projection::screen` of a tile-space point is in the same
space. A floor tile is 80 × 36; the pixels of it that belong to square `q`
are exactly those for which `pick_square` returns `q` (a parallelogram
that fills the box corner to corner), so tiles cut that way cover the map
with no gap and no overlap.

**Ground-relative height.** An object's storey coordinate is measured from
the terrain under its own origin: pass `[x_west, y_north, ground_z]` as the
projection anchor for that object. The map is flat; the terrain's relief
survives only as shading.

**What a mod layer can do** at the pinned revision decides the output
shape: a layer file replaces the base file whole, with no merging. So the
layer ships full copies of every list it extends (art lists, prototype
lists, prototype message files), built from the user's own base files plus
the new entries. An art list holds 4,096 entries **including** the base
game's (about 1,670 walls and 1,860 scenery in a stock install). Maps are
registered in the manifest's `[[maps]]`.

**Per-game slicer profile.** `mojave_convert::map::slicer_defaults(game)`
is the `SlicerConfig` every map run starts from: New Vegas gets
`SlicerConfig::default()` unchanged; Fallout 3 replaces the pattern lists
(`relief_patterns`, `flat_patterns`, `sprite_patterns`, `name_prefixes`)
with the Capital Wasteland's, and also sets `skip_blend_shapes` (leave out
shapes that blend onto the scene: untextured multiply planes and additive
glows, which draw as blank white planes) and `skip_flat_fills` (leave out
shapes whose texture coordinates sample one texel). Nothing else differs.

**Declutter wiring** (2026-11-23). `SlicerConfig::declutter` is `None`
unless the run's profile (`Inputs::declutter_profile`: `--declutter`,
else the slice's `[declutter] profile`, else
`mojave_convert::declutter::default_profile(game)`, `off` for both games)
is `readable`. Then it is `mojave_convert::declutter::profile`: the game's
readable preset, the slice's `[declutter]` and `[tone]` keys over it, and
`protected_refs` = `Records::referenced_refs()` (all of the plugin list's;
the slicer matches only those it is handed). `BuildOptions::interactive`
is filled in the same run only: those references, every window reference
whose base kind is in `protected_kinds` or the default list (every item kind, so an item `protect_items = false` leaves standing stays interactive), and every teleport door. The
profile name is in the map stage's key only when it is not `off`; the
slice tables are in the slice's TOML, which is in the key already.

### 9.1 `map-terrain`

```rust
pub struct Overlay<'a> { pub origin: [i32; 2], pub width: u32, pub height: u32, pub rgba: &'a [u8] } // map screen space
pub struct TerrainOptions { pub library_cap: usize, pub dither_strength: f32, pub relief: f32, pub jobs: usize }
pub struct Ground {
    pub tiles: Vec<Vec<u8>>,   // the tile library: 80 × 36 indexed images, index 0 outside the parallelogram
    pub squares: Vec<u16>,     // 10,000 entries: library index for each square, numbered as HexGrid does
    pub stats: GroundStats,
}
pub fn build_ground(mir: &Mir, worldspace: &str, window: &MapWindow, sampler: &GroundSampler,
                    projection: &Projection, calibration: &Calibration, map: &PaletteMap,
                    overlays: &[Overlay], options: &TerrainOptions) -> Result<Ground, TerrainError>;
```

Ground colour is evaluated per pixel: unproject the pixel to the ground,
blend the land texture layers there, multiply by vertex colour, shade from
the terrain normal, composite the overlays, quantize with ordered dither at
absolute coordinates, then deduplicate the 10,000 tiles into at most
`library_cap`.

### 9.2 `map-slicer`

```rust
pub enum ArtCategory { Walls, Scenery, Items }
pub enum Role { WallStrip, Scenery, Door, Container, Item }
pub struct Budget { pub walls: usize, pub scenery: usize, pub items: usize }   // free entries per list
pub struct ArtImage { pub category: ArtCategory, pub width: u32, pub height: u32, pub pixels: Vec<u8>,
                      pub shift: [i16; 2], pub source: String }
pub struct Placement { pub art: usize, pub hex: i32, pub displacement: [i32; 2], pub role: Role,
                       pub reference: String /* form key */, pub blocking: bool }
pub struct BakedOverlay { pub origin: [i32; 2], pub width: u32, pub height: u32, pub rgba: Vec<u8> }
pub struct Sliced { pub art: Vec<ArtImage>, pub placements: Vec<Placement>, pub overlays: Vec<BakedOverlay>,
                    pub blocked_hexes: Vec<i32>, pub skipped: Vec<Skipped>, pub report: BudgetReport }
pub fn slice_statics(mir: &Mir, references: &[mir::Reference], window: &MapWindow, sampler: &GroundSampler,
                     projection: &Projection, renderer: &mut dyn Renderer, map: &PaletteMap,
                     budget: &Budget, config: &SlicerConfig) -> Result<Sliced, SliceError>;
```

Every reference is classified: skipped (effects, markers, disabled, no
mesh), baked into the ground (flat meshes flattened; rocks and cliffs
rendered in relief, their footprint returned in `blocked_hexes`), a single
sprite, or sliced into per-hex strips. A strip holds the pixels whose
surface point, found through the depth buffer, stands over that hex.
Exceeding `budget` is `SliceError::FidBudget`, whose message names the
list, the cap, the demand and the largest contributors.

### 9.3 `map-builder`

```rust
pub struct BuildInput<'a> { /* map name and number, window, sampler, projection, the ground,
                              the slicer's art and placements and blocked hexes, the cells (for navmeshes
                              and teleports), the base game's files, options */ }
pub struct Built { pub layer: emit::ModLayer, pub walkable: Vec<bool> /* 40,000 */, pub report: BuildReport }
pub fn build_map(input: &BuildInput) -> Result<Built, BuildError>;
pub fn base_file_paths() -> &'static [&'static str];   // what the caller should read from the Fallout 2 install
```

A hex is walkable when a navmesh triangle covers it, the terrain there is
under the slope limit, and nothing blocks it. Hexes that are not walkable
and hold no blocking object get an invisible blocker. A teleport door
becomes a door object and an exit that leads to a stub map of its own,
standing in for the interior it opens on, and back to that same door.

### 9.4 As built

- Under Fallout 2 the engine resolves a map's number through
  `data/maps.txt`, not through the manifest, so the layer ships a full
  `maps.txt` (the base file plus the new records) and registers the maps in
  the manifest as well. Without the base file, exits cannot resolve.
- Only critter, scenery and wall art blocks movement. Containers and items
  get an invisible blocker beside them.
- Floor tile indices 0 and 1 are reserved (1 means blank); an empty tile
  list is padded so that real tiles start at 2.
- Exit grids are the misc prototypes 16 to 23.
- Objects are drawn in ascending hex number, row by row. Strip slicing and
  the map preview both rely on that order.
- Containers are item prototypes, so their art goes in the items list.
- A "flattened" ground overlay is rendered with its height scaled by 1e-3,
  since the renderer refuses a singular transform.
- Square ownership is offset from integer tile coordinates: use
  `pick_square`, never `floor(s)`.
- `MapWindow::tile` ignores `yaw_quarter_turns`; `Projection` applies it.
  The map stage has only been run at yaw 0.
- A sliced object blocks the hexes within its bounding footprint, except
  those in `SlicerConfig::open_hexes`, which also never merge into a run.
  The map stage fills them from `map_builder::walk::standable_hexes` (the
  navmesh and the slope limit, in cells that have a navmesh). A run that
  extends the base lists keeps only hexes within 6 steps of a teleporting
  door (`DOOR_APPROACH`): opened everywhere, the strips that can no longer
  merge overflow its walls list. A standalone run opens every standable
  hex (`mojave_convert::map::OpenPolicy`); its budget ladder pays for the
  extra strips. `map-report.md`'s `## Open hexes` gives the policy and the
  counts.
- The map stage hands the slicer the references whose origin lies in the
  window. A standalone run also hands it every static (`STAT`, `SCOL`)
  whose origin lies outside but whose placed bounding footprint overlaps
  the window: left out, a control tower or a hotel wing straddling the
  edge leaves the ground under it bare (at Nellis painted black in the
  source). The slicer drops their pixels off the map.
- `SlicerConfig::landmark_walkers` is every hex `map_builder::walk::verdicts`
  calls walkable with nothing blocked (navmesh and slope, and slope alone
  in cells without a navmesh), in both modes.
- `BuildOptions::protected_hexes` is every hex of the slicer's merged strip
  runs (`Placement::run`, first to last hex of its row). The slicer does
  not report a landmark's blocked core apart from relief in
  `Sliced::blocked_hexes`, so that core is left to the builder's own
  test (`cover::Cover::stands_on`).
- Walking surfaces of a sliced object near an open hex (facing up, 8 to
  80 units above the terrain) are not strip pixels. They are placed as
  `Role::FloorPiece`: scenery art with `FLAT | NONBLOCKING |
  NO_TRANSLUCENCY`, which the engine draws under every character and never
  cuts with the see-through circle. A strip keeps posts, railings, roofs.
- The engine's circle cuts scenery and wall art alike; only a
  translucency bit on the object exempts it, and `FLAT` alone does not.
- A base prototype list does not always name its files by position; a new
  prototype whose conventional name is taken gets `mfNNNNNN.pro`.

As built for the demonstration maps (2026-11-22):

- **Standalone mode** (owner decision, 2026-11-22, for demonstration
  layers only): `map_builder::BuildOptions::standalone`, set by
  `mojave-convert map --standalone` and `all --standalone`. The layer's
  `art/walls/walls.lst` starts empty and `art/tiles/tiles.lst` starts with
  its two reserved entries only, so about 4,096 and 4,094 entries are the
  map's. Every other art list, the prototype lists and the message files
  are extended as before. Stock maps draw the wrong walls and floors while
  such a layer is mounted. `verify_layer` accepts the two replaced lists
  and names them in `VerifyReport::replaced_art_lists`; any other list
  that drops base entries is still refused.
- **Budgets.** `map_builder::art_budget(base_files, standalone)` gives, per
  art list, the entries kept, free, and the builder's own share
  (`available()` is the caller's). A standalone run gives the slicer and
  the terrain library exactly `available()`. A run without the flag keeps
  the numbers it always had (free entries less 8, tiles capped at 2,000),
  computed from the same `art_budget`; a unit test pins the equality, and
  the real Goodsprings layer is byte-identical. The mode is part of the map
  stage's cache key only when set, so earlier keys stay valid.
- **Terrain method.** `TerrainOptions::library_method`: standalone runs use
  `LibraryMethod::GreedyMerge` (lower error than k-means at every cap
  measured); other runs keep the default `KMeans`, and with it their bytes.
- **Floor tile 1** is verified against the engine's rules: a cleared tile
  pair is `(1, 1)` and roof index 1 is never drawn, while floor index 1 is
  drawn like any other floor. Entries 0 and 1 are what a standalone tile
  list keeps (`emit::RESERVED_TILE_ENTRIES`).
- **Landmarks.** A mesh above `max_sliced_hexes` (2,500) that matches no
  relief or terrain pattern is a landmark, `ClassKind::Landmark`: rendered
  upright and placed as scenery **pieces** (not bands) of at most 512 x 352
  pixels, at most 160 per landmark. The engine's 640 x 480 view draws an
  object only when its anchor hex is within a margin around the screen,
  so each piece is anchored near its own pixels, at a hex late enough in
  the draw order that no walker is drawn over a wall; hexes where that
  cannot hold are sealed. `LandmarkStats` counts `pieces`, `tiles` (pieces
  of the visibility cut), `visibility_splits`, `sorting_splits`,
  `walkers`, `hidden_walkers`, `sealed_hexes` and `unresolved_pieces`;
  `map-report.md`'s `## Landmarks` lists them. A landmark that cannot be
  drawn upright falls back to relief (`LandmarkStats::fallbacks`).
- **Default land texture.** Land with no base layer, or a layer naming no
  texture, draws with the game's default land texture (`mir`,
  `map-terrain`); `GroundStats` counts `defaulted_texture_pixels`,
  `defaulted_texture_layers`, `unnamed_texture_layers` and
  `borrowed_base_layers`, summarised in `map-report.md`'s `## Ground`. It
  changed 46 of the real Goodsprings map's 10,000 squares (15 squares
  re-pointed, 6 tile images redrawn), and nothing else in its layer
  (`out/baseline-goodsprings-3.sha`).
- **Reach.** `map_builder` reports the entrance (`BuildReport::entrance`:
  hex, how a figure there is drawn, whether it is in the largest
  component, steps from the window centre), the written map's components
  (`components`, with `unreachable_exits()` and `stranded_returns()`),
  passages opened to islands holding an exit, a return or a critter
  (`passages`, `opened_hexes`), islands left with the reason
  (`islands_left`), and each exit's `grid_steps`. `map-report.md` has
  `## Entrance`, `## Components`, `## Passages` and `## Islands left`, and
  each exit line gives its grid steps and hexes.
- **Budget ladder** (`SlicerConfig::ladder`, on by default; steps taken in
  `BudgetReport::ladder`): when a list would overflow, (1) perimeter runs
  merge up to 8 hexes, (2) strips under 192 pixels join a neighbour, (3)
  the largest sliced models become landmarks, (4) the smallest
  contributors are baked into the ground. Each step re-renders the window;
  a run that fits at once is untouched. `map_slicer::estimate` is the fast
  pre-check (`mojave-convert estimate`) and never climbs the ladder.
- **Pattern rules.** Model path patterns match path segments and word
  starts, not substrings: `dir\` a whole directory, `\name` a segment
  start, `*name` anywhere, a plain `name` at a word start (also after the
  name prefixes `nv` and `scol`), `!text` an exclusion. So `rock` matches
  `nv_rocks\` but not `shamrock.nif`, and `!rocket` keeps rockets upright.
- **Trigger volumes.** A reference whose base is an invisible trigger or
  activator volume (`mir::Reference::trigger_volume`) is skipped,
  `SkipReason::TriggerVolume`; `map-report.md` lists them.
- **Report and features.** `map-report.md` gains `## Art lists` (mode,
  budget given, per list replaced, base, kept, added, free before and
  after), `## Budget ladder`, `## Landmarks` and `## Trigger volumes
  skipped`. Beside it, `map-features.json` lists the map's largest
  landmarks and wall-strip objects: the four largest landmarks first, then
  all by placed pixels (`mojave_convert::map::rank_features`, which
  `shots` also applies to a file it reads), with the hex under the centre of their drawn extent (or, when
  that centre is off the grid, as for a tower cut by the map's edge, the
  hex of the piece nearest it); `mojave-convert shots --auto` aims at
  them, and takes a model a second time only when every other model has
  had its turn. Neither file is in the layer. The map stage's cache key
  is `map-8` (it was `map-7` before interior slices).

### 9.5 Interiors (2026-11-22)

An interior slice (`SliceSpec::interior`, the cell's editor id) builds one
interior cell as a standalone demo map: one layer, one map, one storey,
no exits. `mojave-convert` refuses it without `--standalone`
(`Inputs::check_mode`, before extract). The chain, in `map.rs`
(`interior_scene`):

- **Cells and references.** The one interior document
  (`Mir::interior`), every reference whose origin lies in the window (and
  statics whose footprint reaches in). `x`, `y` and `worldspace` only say
  what to extract; an unextracted room is an error naming the interiors
  that were extracted.
- **Floor.** `floor_z` from the slice, else `mir::storeys(&doc)[0]` (the
  largest storey), else `map_builder::dominant_floor_z`. The ground is
  `GroundSampler::flat(floor_z)`.
- **Window.** Centred on `map_center` (room coordinates), else
  `map_builder::navmesh_centre` of the floor band, else the centre of the
  references' origins.
- **Slicer.** `SlicerConfig::interior = SliceSpec::interior_slice(floor_z)`
  (`InteriorSlice::at` with `cut_height`, `floor_band`, `hang_clearance`
  overridden); `open_hexes` is every hex `walk::standable_hexes` gives
  with the interior build options (`OpenPolicy::Everywhere`);
  `landmark_walkers` as outdoors.
- **Terrain.** `build_ground_from(&MemoryLand::default(), ...)` with
  `blank_without_land`: the room's coordinates must never read the
  worldspace's land. Every square `Ground::is_blank` reports becomes
  `map_builder::BLANK_SQUARE` in `GroundTiles::squares`.
- **Builder.** `BuildOptions::interior = SliceSpec::interior_build(floor_z)`
  (`walk_band` `[low, high]` as `below = -low`, `above = high`;
  `clamp_levels`), `entrance_near` = the slice's `entrance`, else the
  position of the room's teleport door with the lowest form key; the
  builder's interiors map (for exits) is not filled.
- **People.** `critter_inputs`: with `place_disabled` the reference's
  `initially_disabled` flag and its enable parent are ignored (deleted
  references never place); several references to one actor each get a
  placement; an actor with fewer than 1 hit point is left out as dead in
  the data (an exterior still lifts it to 1); creatures are left out and
  reported; a name several placements share gets an article in the
  description ("You see a Powder Ganger."). Everyone left out comes back
  as `PersonSkip` (name, reference, reason).
- **Report and features.** `map-report.md` opens with `## Interior`:
  cell, floor z and its source, walk band, window centre, entrance target,
  every storey, cut settings and `BudgetReport::interior` tallies,
  `BuildReport::interior_summary()`, people placed (name, reference, hex)
  and left out (with the builder's skips). `map-features.json` gains
  `interior` (the editor id) and `people` (name, reference, hex, ranked
  by `rank_people`: names of their own first); both are absent on an
  exterior map, whose file is unchanged.
- **Shots.** On a map whose features name a room, `--auto` aims at the
  entrance, then the people (`people_features`, kind `person`, the name as
  the model), then the walls; a person's viewpoint needs only
  `PERSON_SPREAD` (1) step from the others, the rest `INTERIOR_SPREAD`
  (5) unless `--spread` is given, and the wide view is captured at
  `INTERIOR_WIDE_ZOOM` (0.75, a frame about 1138 x 507) unless
  `--wide-zoom` is given. The engine draws the wide view at its own pixel
  scale whatever the zoom: 0.5 gives 1707 x 760 pixels, 1.0 854 x 380.
- **Cache.** The slice's TOML (every new key) is in the key; an exterior
  slice serialises none of them.

### 9.6 Items, loot and sides (2026-11-23)

With loot on (`Inputs::loot_on`: `--loot on|off`, else the slice's
`loot = true`, else off) the map stage builds a `map_builder::Catalogue`
(`mojave_convert::catalogue::build`) after slicing and hands it to the
builder in `BuildOptions::catalogue`. Off, nothing below runs and the
layer is what it was. The rules are in the module notes of
`crates/mojave-convert/src/catalogue.rs`; in short:

- **Rebalance tables** (`mojave_convert::rebalance::LoadedRebalance`):
  `emit::rebalance::Rebalance::for_game(game)` and the converter's
  `ConvertTables` (`[convert.weapons]`, `[convert.hostility]`,
  `[convert.loot]`), overlaid by `--rebalance <file>`, else
  `rebalance/<game>.toml` in the slice file's directory or above (the
  working directory for a built-in slice), else built in. The repository's
  `rebalance/fo3.toml` and `fnv.toml` are full copies of the defaults
  (`MOJAVE_BLESS=1 cargo test -p mojave-convert --lib rebalance` rewrites
  them). `LoadedRebalance::digest` (the effective tables, not the file) is
  in the map stage's key when loot is on, with `loot=on`.
- **Items** (`Catalogue::items`, keyed by the base's form key): every base
  of a reference the slicer placed as an item only (`Role::Item`), of the
  contents of every reference placed as a container only, and of the
  inventory of every person the builder is given (the NPC that supplies
  inventory through its templates, flag 0x100), plus every weapon's
  ammunition. Records: `weapons` (with the `DNAM` automatic flag from the
  raw store), `armors` (armour rating in hundredths: DR = rating / 100),
  `ammo` (calibre key from the name), `ingestibles` (effects from `EFID`
  and `EFIT`, by the magic effect's editor id and actor value; addiction
  from `ENIT`), `misc_items` and the base object for keys, books, notes and
  the rest (all miscellaneous). Leveled item lists (raw `LVLO`) are drawn
  per container, seeded by the container reference's key; a person's by
  the actor documents' rule.
- **Art**: `GroundArt::FirstPlacement` for an item some placed reference
  draws; otherwise `GroundArt::Sprite` of the model rendered as the sprite
  stage renders a prop (the extraction's mesh, else read from the install
  through `mir::DataSource`), cached in `<out>/sprites/` under the sprite
  stage's key scheme (`sprites::key_prefix`); the inventory icon
  (`ItemIcon::Indexed`) is the same model rendered three times larger
  (`tile_metres / 3`, `pixels_per_metre_vertical * 3`), cached the same
  way. A model that does not render gets a small placeholder box and no
  icon (reported).
- **Placed** (`Catalogue::placed`): `Item` (count from `XCNT`; ammunition
  in packs, the front pack partly full), `Container` (base name, capacity
  at least 100 and above the contents' size, `LockState { locked: lock
  present, jammed: false }`, at most `max_contents` stacks), `Door` (base
  name, lock, the lock's key when the catalogue holds that key), `Named`
  for activators, terminals, furniture, moving statics and sliced doors or
  containers with a real name.
- **People** (`Catalogue::critters`, by reference): `fighter_spec` of a
  `SourceActor` (level and health from the actor document; attributes and
  skills from the stats supplier's `DATA` and `DNAM`) with the worn armour
  (the first body armour, slot 0x4, of the inventory); right hand =
  `ActorDoc::weapons[0]` when carried (the weapon the critter stage drew);
  carried = the rest (at most `max_carried`). Side by
  `rebalance::Hostility`: AI data (`AIDT` aggression, after templates by
  flag 0x10) and factions (`SNAM`, flag 0x04) against the player's
  faction's relations (`XNAM`, both ways); patterns only when the record
  has no AI data. Hostile people get `hostile_team` (66) and the packet
  named `hostile_packet` in the base `data/ai.txt` ("Merc Raider", 136 in
  a stock install); friendly people the builder's defaults (team 99, the
  packet of `critters::choose_ai_packet`).
- **Footprints**: the slice key `footprints = true` sets
  `SlicerConfig::furniture = Some(FurnitureRule::default())` and passes
  `Sliced::footprints` to `BuildOptions::footprints`; off by default.
- **Report**: `map-report.md` has `## Loot` (`LootReport::loot_summary`,
  `CatalogueReport::lines`, the hostile and friendly split, a table of
  people and one of items) and `## Furniture`
  (`FurnitureReport::summary`); `## Characters` lists each sprite set's
  row, files and bytes and the highest `critters.lst` row used.
  `gallery/items-NN.png` shows each item's ground sprite beside its icon.

### 9.7 Layers, links, storeys and levels (2026-11-23)

The map stage builds several maps in one layer through
`map_builder::build_layer` when the run has a layer
(`Inputs::layer_spec`: a `*.layer.toml` given to `--slice`,
`--with-interiors` on an exterior slice, or a slice with `storeys = "auto"`
or `levels = "auto"`, a layer of one map). Otherwise `map::map` is the
one-map path of 9.4 to 9.6, byte for byte. The code is
`mojave_convert::layer`; the file format, keys and commands are in
`docs/running.md` ("Layers").

- **Layer file** (`layer::LayerSpec`): `name`, `start`, `with_interiors`,
  `[all_maps]` (slice keys laid over every map) and `[[maps]]` (`slice` =
  a slice file relative to the layer file or a built-in name, or
  `interior` = an editor id taking the first exterior's keys; any other key
  overrides the slice's). Maps share one game, plugin list and worldspace;
  stems are distinct. `LayerSpec::union_slice` is what extract, sprites
  and critters see (the union of the cell ranges, the deepest interior
  depth, loot on when any map has it, the first map's critter keys).
  `layer::actor_ids` is the critter stage's `only` above minimal.
- **Plan.** One `MapRun` per map, one `Run` per elevation (one slicer
  run, one terrain run). Exterior: `map::scene`, then with levels
  `split_levels` over the window's cells (`LevelOptions::default`),
  `ElevationInput::level = exterior_level(k)`, the open hexes and landmark
  walkers from `walk::standable_hexes` / `verdicts` with
  `BuildOptions::level`. Level 0 slices every reference with the
  worldspace sampler; level `k > 0` slices the references whose
  `reference_level` is `k` (doors by `level_of_point`) with an empty
  sampler (each reference anchored on its own origin), and bakes its decks
  flat (`SlicerConfig::flat_references`: per triangle of the level, the
  reference with the smallest placed footprint, at least 8,000 units²,
  holding its centroid in plan and within 40 units in height; two or more
  triangles). Its ground is `build_ground_from(MemoryLand::default())`
  with `blank_without_land`. Interior: `map::interior_scene`, then with
  storeys `split_storeys` and floors grouped into storeys (a floor whose
  height lies in a larger floor's band joins it; the three largest kept,
  or `storey_floors`), per storey `SliceSpec::interior_build(z)` and
  `interior_slice(z)` with the cut capped 10 units under the next storey,
  `GroundSampler::flat(z)`, the storey's own open hexes. Stairs: per
  connector, its landings on two kept storeys, as `StairLink`s of the map
  with itself (labels `<stem> stairs a-b`); levels give
  `LevelSplit::stair_links`. A teleport door's placement stays on the
  elevation its height (or level) gives and is dropped on the others.
- **Entrance.** `BuildOptions::entrance_near` from an exterior's
  `entrance` key; with levels and no key, the window's first door leading
  to another worldspace; `MapInput::entrance_elevation` its level. An
  interior's entrance storey is the one nearest its first door's height.
- **Teleports.** A teleport whose cell no map of the layer is built from
  (a door filed in the worldspace's persistent cell) is pointed at the
  cell its partner door was extracted into before building
  (`normalise_teleports`); the report counts them.
- **Budget.** `map_slicer::estimate` per run; shares of
  `Budgets::slicer` in proportion (largest remainder, `layer::shares`);
  slicing order interiors, upper levels, ground levels, each run passing
  on what it leaves; tiles in proportion to each run's floor (an exterior
  ground level 10,000) smallest first with carry-over. The builder checks
  the layer's lists once.
- **Catalogue.** One `LayerOptions::catalogue` from one
  `catalogue::build` over every map's references, every run's art and
  placements (art indices offset) and every map's people. Critter sprite
  sets are shared (`map::critter_inputs_into`).
- **Checks.** The written layer is read back, `verify_layer` and
  `verify_links` (every grid onto a walkable hex of an existing elevation;
  one-way links listed) run on it; the report's `## Layer` gives both.
- **Furniture, wire, door stubs, coverage, sight.** `footprints = true`
  as 9.6 (per map, the footprints of all its elevations); `wire = true`
  sets `SlicerConfig::wire` (`map::wire_rule`: the slicer's defaults plus
  the cart and wire-basket patterns) and takes `*shoppingcart` off the
  readable preset's clutter patterns; door stubs, the cutaway wall rules
  and the roof cut are the readable preset's. `navmesh_coverage` sets
  `BuildOptions::navmesh_coverage` (`share:0.5` with footprints indoors),
  `sight` / `entrance_sight` `readability_sight` / `entrance_sight`
  (`visible` = `SightRule::VISIBLE`). All of them apply on the one-map path
  too, only when present. `[tone.floor]` with the pass on also tones the
  terrain (`TerrainOptions::tone`).
- **Report and features.** `map-report.md`: `## Layer` (maps, links both
  ways and one way, stubs, `verify_links`, the budget per map and
  elevation), the art lists, loot and characters once, then per map its
  storeys or levels and per elevation reach (components, entrance,
  islands left, passages, arrivals, readability), the slicer, ground and
  open-hex sections. `features/<stem>.json` per map (the entrance
  elevation's features and people); `map-features.json` is the start
  map's. Cache: `cache/map.key` is the layer's key (the layer file, the
  stems, and `map::cache_key` of every map's inputs); `cache/maps/<stem>.key`
  each map's.
- **Placement index.** On both paths (`layer`, and `map::map` for one map)
  the stage writes `map-placements.json` beside `map-report.md`: every placed
  critter, door, container, item and terminal with its map, elevation, hex,
  object id and PID as written, plus the references of the maps' cells that
  were left off, each with the stage's reason, and the stub maps listed with
  no references. Schema in section 11.2. The stage reuses its output only
  when this file is present (`placements::is_current`: it reads, is of this
  schema version and has origin `stage`). Each entry is joined to its object
  by the builder's exact reference-to-object map (below), not by position,
  and read back against the written maps; the file's notes count any
  disagreement.
- **Pass 2 options (2026-11-24; spec 7.6, 8.3).** Three keys, all off or
  as before by default, change what the map stage writes so that the `sim`
  stage has something to attach scripts to:
  - `enable_groups = true` (slice key, `config::SliceSpec::enable_groups`, default off;
    on a layer it applies per map, and `Inputs::enable_groups_on()` is what
    the stages ask). For every non-actor member of an enable group in the
    scene's cells (a reference with an enable parent, or one that is a
    parent; chains are flattened to their root by
    `placements::group_members`, cycles cut), `map::apply_enable_groups`
    hands the slicer the reference as the group's initial state says: a
    member that starts enabled is handed on enabled (a child follows its
    parent, its own flag is not read); one that starts disabled is handed on
    enabled too and written hidden (`BuildOptions::hidden`), but only when the
    slicer would draw it as an object (a sprite, strips or a landmark). A
    member the slicer would bake into the ground, or skip (a marker, a
    light), stays off the map and is listed (`GroupsReport::left`, reason
    "starts disabled; ..."; the index lists it as `unplaced`, reason
    `skipped`). Actors are the critter pass's: a group member that is an
    actor follows its group there (`map::` people placement): placed hidden
    when the group starts it disabled, placed as usual when the group starts
    it enabled whatever its own flag says. What the pass changed is kept in
    `GroupsReport::originals` (the source's own flag and enable parent,
    `placements::Original`), so the index keeps describing the source. The
    report has `## Enable groups`.
  - `presence_copies = true` (layer file key, `LayerSpec::presence_copies`,
    spec 7.6). A named actor whose AI packages lead to another map of the
    layer gets one hidden copy on that map (`layer::add_presence_copies`):
    from the first package of its list whose location is understood, a
    reference (type 0), a cell (type 1, near the centre of the map built from
    that cell) or the actor's own linked reference (type 6); "near the editor
    location" (3) leads nowhere. The copy shares the original's sprite set,
    name and figures, is `CritterPlacement::hidden` with
    `CritterPlacement::presence_of`, and is placed on the nearest walkable hex
    to the location, clear of every link's keep-clear hexes. None where the
    map already places a reference to the actor. The index lists the copy as
    a placed entry whose `reference` is `<original>@<map stem>` and
    `presence_of` is the original's. The key is part of the layer's cache key.
  - `hostile_packet = <n>` (slice key): the AI packet number hostile people
    take when `loot` is on, instead of the rebalance table's (the packet
    named by `convert.hostility.hostile_packet`, "Merc Raider" by default,
    then `hostile_packet_number`, 136). It must be a packet of the base
    `ai.txt`: another number is a `ConvertError::Slice` that stops the stage.
    The index marks each critter the catalogue made hostile with `hostile`.
- **Hidden objects.** `BuildOptions::hidden` names placements, by reference,
  that are written with the hidden flag `0x01`. The planner ignores a hidden
  placement entirely: it keeps its prototype's blocking and light flags (the
  engine's movement rule skips hidden objects), but it blocks no hex, opens
  no door grid, covers no figure and counts for no readability, footprint or
  reach figure. It is written after every other object of its elevation and
  is still moved off link hexes. A hidden critter
  (`CritterPlacement::hidden`) is placed after the visible critters on the
  nearest walkable hex within three times `critters::CRITTER_RADIUS` steps,
  clear of exit grids, doors, the entrance, arrivals, passages, keep-clear
  hexes and every visible critter; it is a bare critter (its catalogue entry
  is ignored), listed in `BuildReport::hidden_critters` instead of
  `BuildReport::critters`, so it is no perch, no critter lost to closing and
  no person of the map. `BuildReport::hidden_objects` lists the other hidden
  objects as (reference, elevation, hex), in write order.
- **The reference-to-object map** (`map_builder::BuiltMap::objects`, and
  `Built::objects` on the one-map path). Exactly the objects the map's
  placements and critters became, in write order: `ObjectRef { reference,
  elevation, id, hex, pid, hidden }`, where `reference` is the source
  reference (a placement's, a critter's, a presence copy's own key), `id` the
  object's id in the map file, and `hidden` whether it carries the flag. Exit
  grids, blockers and inventory records are not listed. A reference that became
  several objects (a landmark's pieces, ammunition packs) has an entry for
  each. The placement index is made from this list and read back against the
  written maps, never from positions; a mismatch is counted in the notes.
- **Reach** (`map_builder::reach`). Every walkable component of an
  elevation other than the main one is diagnosed against the elevation's own
  ground rule with nothing blocked (`reach::IslandDiagnosis`, in
  `ElevationReport::islands`): whether the ground joins it to the main
  component (and on the shortest way, each hex that is not walkable and
  what keeps it so), whether the tolerance coverage rule would, what it
  holds, and whether it is reached from the layer's start through the
  links (`reach::layer_reach`). `ElevationReport::reached` is (hexes
  reached, planned walkable hexes); `ElevationReport::closed` lists the
  components closed as unreached. `LayerOptions::close_unreached` (layer
  file `close_unreached`, default on since 2026-11-24; on a one-map build
  `BuildOptions::close_unreached` / the slice key, default on) closes every
  walkable component no walk from the first map's entrance reaches (the
  start map comes first), stepping on an exit grid continuing from its
  arrival hex, except a component a critter would stand on: before closing,
  the critters are placed by the ordinary rule on the grid as it is and any
  unreached component that receives one is kept walkable and listed as an
  occupied perch (`ElevationReport::perches`: size, lowest hex, names; the
  report line "occupied perch: N hexes at hex H, holds X (not reached from
  the start)"). A critter placed before closing is placed after it:
  `ElevationReport::lost_to_closing` is empty, and the report says so.
  Empty unreached components close; the report counts them. Both are computed from the planned grids before anything is
  written.
- **Exterior ground options.** `BuildOptions::navmesh_only` (slice key
  `navmesh_only`, exterior only, default off): a hex in a cell without a
  navmesh is not walkable when some other cell of the window has one
  (`Verdict::NoGround`), instead of the slope rule; Megaton's ground
  outside its wall. `BuildOptions::coverage_bridges` (slice key
  `coverage_bridges`, default on since 2026-11-24): under a `navmesh_coverage` stricter than
  the tolerance rule, the islands it cuts out of one floor are joined to the
  larger part through the hexes the tolerance rule covers, at most
  `walk::BRIDGE_MAX_LENGTH` (8) hexes per join (`walk::bridge_coverage`).
  `TerrainOptions::vertex_color_floor` (slice key `vertex_color_floor`, 0 to
  1, default 0): the darkest a land vertex colour may make the ground.
- **Storeys walk their own triangles.** The converter gives each storey's
  `InteriorBuild::triangles` the member triangles of the floors grouped into
  it (`Floor::members`), so walkability takes exactly those at any height
  instead of every triangle inside the band; a floor that dips at a stair
  head stays one floor. `None` keeps the band, as every earlier build.
- **Link clearance** (`map_builder::clearance`). After the walkable grids
  are final (passages opened, unreached components closed) and before
  critters and loot are placed, nothing is left on or by a link's exit
  grids: the keep-clear set of an elevation is every exit grid hex of its
  door and stair links and every arrival hex another link leads to here
  (`DOOR_KEEP_CLEAR_RADIUS` 0), plus every hex within
  `STAIRS_KEEP_CLEAR_RADIUS` (1) of a stair's grid hexes. A ground item, and
  a container that does not block its hex, in the set moves to the nearest
  free walkable hex outside it (same walkable component when it has room,
  ties to the lowest hex; `ElevationPlan::moved_off_links`, reported as
  `ElevationReport::moved_off_links` and the note "moved off link hexes:
  N"); critters are placed clear of the whole set. A container that blocks
  its hex stays. Stub exits are not links, so a layer without links is
  byte-identical. `verify_clearance` (run by `build_layer` after
  `verify_links`) checks the written maps: no category 0 or 1 object (item,
  container, critter) on a grid or arrival hex, and none but a blocking
  container within the stairs radius.
- **Slicer addition.** `SlicerConfig::flat_references` (form keys always
  flattened onto the ground, items, doors and containers excepted; empty
  by default, nothing changes).
- **Walk tools.** `route::WalkMap::load_elevation`, `Exit::to_elevation`;
  `probe walk --elevation`; `replay::Hop::lands` (a hop ending on an exit
  grid; the camera plan re-centres after it); `sequence::walk_hops_in`.

## 10. Characters (M4)

Written 2026-11-20, before any M4 code. Engine rules below were read in
`vault-engine` at `df2bf4c` and its approved specs; source facts in the
real Goodsprings extraction.

### 10.1 What the engine requires of a critter

- **Art name.** A critter FID is category 1; bits 0 to 11 are the row in
  `art/critters/critters.lst`, bits 12 to 15 a weapon code, bits 16 to 23
  an animation code. The file is `<stem><two letters>.frm`, the stem up to
  12 ASCII bytes (6 by convention). Rows above 4,095 cannot be addressed.
- **A list line** is `stem,alias`. The alias is a row whose art is
  borrowed for a few special deaths; without one it is row 0. A new
  critter names its own row.
- **Always `.frm`, six directions in one file**, in the order NE, E, SE,
  SW, W, NW. Per-direction `.fr0` to `.fr5` files are tried only for
  fall and death animations.
- **Animations by need** (unarmed):

  | Code | Suffix | What | Without it |
  |---|---|---|---|
  | 0 | `aa` | stand, also the idle fidget | the critter is invisible |
  | 1 | `ab` | walk | invisible while walking |
  | 14 | `ao` | hit | an attack on it is thrown away |
  | 20, 21 | `ba`, `bb` | fall back, fall front | knockdown and death fail |
  | 36, 37 | `ch`, `cj` | get up | stays down |
  | 48, 49 | `ra`, `rb` | corpse | no body after an instant kill |
  | 16, 17 | `aq`, `ar` | punch, kick | it cannot fight back |
  | 13 | `an` | dodge | a missed shot from a weapon without a projectile registers the dodge without testing the art, and the whole attack fails: **required** (corrected 2026-11-23; `emit::combat_requirements` has it required, unarmed and per weapon code) |
  | 15, 19 | `ap`, `at` | hit from behind, run | engine falls back |

- **Frames.** Every direction has the same frame count. Stored fps 0
  means 10. The frame's bottom centre sits at the hex centre plus the
  direction shift plus the accumulated per-frame offsets.
- **Stand** frames carry zero offsets.
- **Walk** frames carry offsets toward that facing's neighbour hex. The
  critter moves to the next hex when the accumulated offset reaches the
  hex step on either axis; the steps are NE (16, -12), E (32, 0),
  SE (16, 12), SW (-16, 12), W (-32, 0), NW (-16, -12). A direction whose
  offsets never reach its step never finishes a walk.
- **Prototype.** `proto/critters/critters.lst` line N is PID
  `0x01000000 + N`; the file is 416 bytes and its first word is that PID.
  `vault_formats::Prototype::write` writes it. The engine reads the
  message base (name and description in `pro_crit.msg`), stats, skills,
  critter flags, body type (0 or 2 opens doors), experience, kill type.
- **Map record.** 128 bytes before inventory. A placed critter needs a
  unique id, a hex, orientation 0 to 5, frame 0, the stand FID, its PID,
  script id -1, an AI packet number, a team, last attacker -1, status 0,
  and **current HP above 0**: at 0 the engine treats it as dead.
- **Behaviour.** A critter without a script stands and fidgets. Nothing
  makes it wander; it walks only in combat or when a script moves it. It
  joins a fight only when it or its team is attacked. Team 0 is the
  player's. No packet field makes a critter hostile.
- **Layers.** List and message files are replaced whole, as for art: the
  layer ships full copies of both `critters.lst` files and `pro_crit.msg`
  with rows appended. `data/ai.txt` is shipped only if a new packet is
  needed, and then whole.

### 10.2 What the source provides

In the map window: 10 placed townsfolk (two start disabled) and 12
creatures (bighorners, ravens, two disabled mantis nymphs). The named
residents stand in interiors, which are stub maps today. The slice has 23
distinct NPC bases.

- **Present:** every placed actor with base, transform and flags; per
  NPC its race, sex, hair, eyes, template link and inventory; every raw
  subrecord (extraction runs with the raw store on); archive access to
  the one human skeleton, the body, head and hair meshes and every `.kf`.
- **Missing:** nothing reads race body parts, head-part lists, templates
  or leveled lists; `nif` skips skin and animation blocks and discards
  the bone tree; no stage renders more than one still frame.
- The human idle is `meshes\characters\_male\locomotion\mtidle.kf`
  (spline-compressed); the walk is `locomotion\male\mtforward.kf` or
  `locomotion\female\mtforward.kf` (plain keys).

### 10.3 Interfaces

`nif`
```
pub struct Skeleton { pub bones: Vec<Bone> }            // a parent precedes its children
pub struct Bone { pub name: String, pub parent: Option<u32>, pub local: BoneTransform }
pub struct BoneTransform { pub rotation: [[f32; 3]; 3], pub translation: [f32; 3], pub scale: f32 }
pub fn extract_skeleton(file: &NifFile) -> Result<Skeleton, NifError>;

pub struct Skin { pub bones: Vec<SkinBone>, pub weights: Vec<[(u16, f32); 4]> }   // one entry per vertex
pub struct SkinBone { pub name: String, pub bind_inverse: [[f32; 4]; 3] }
// Primitive gains `skin: Option<Skin>`; `skinned` stays true exactly when it is Some.

pub struct Animation { pub name: String, pub duration: f32, pub looping: bool,
                       pub tracks: Vec<Track>, pub text_keys: Vec<(f32, String)> }
pub struct Track { pub bone: String, pub rotations: Vec<(f32, [f32; 4])>,     // time, quaternion wxyz
                   pub translations: Vec<(f32, [f32; 3])>, pub scales: Vec<(f32, f32)> }
pub fn parse_animation(bytes: &[u8]) -> Result<Animation, NifError>;   // plain and B-spline tracks

pub fn pose(skeleton: &Skeleton, animation: &Animation, time: f32) -> Pose;   // world matrix per bone
pub fn skin(mesh: &StaticMesh, skeleton: &Skeleton, pose: &Pose) -> StaticMesh; // posed, no skin left
```
A bone an animation does not name keeps its skeleton transform. Bones
are matched by name. The accumulation root's horizontal translation is
returned apart from the pose (`Pose::travel`), so a walk can be rendered
in place.

As built (`nif`):
- `Animation` also carries `accum_root` (the bone whose horizontal motion
  is travel; `Bip01` in every human animation), and `Pose` is
  `{ world, travel }` with `travel` a field.
- A skinned primitive's vertices are in mesh space at bind; a bone's
  `bind_inverse` maps mesh space to that bone's space at bind. Posed
  position = sum of weight x `pose.world[bone]` x `bind_inverse` x vertex.
- The human skeleton stores `Bip01` at hip height while animations key it
  at the ground, so channels an animation does not key on the
  accumulation root are identity, not the skeleton's values.
- Spline tracks are sampled at 30 Hz into plain keys. Times start at the
  sequence's start; looping animations wrap, others clamp.
- Measured on the real files: 66 bones; the idle is 4.0 s; the male walk
  is a 1.2 s cycle that travels 102.3 units along +Y (85.3 units a
  second); a posed figure is about 127 units tall.
- Errors are `NifError::Animation`. Five `.kf` files of an older file
  version are refused; everything else under the human folder parses.

`mir`
- Skinned meshes stay `meshes/<hash>.glb`, now with standard glTF joint
  and weight attributes and a skin, round-tripped by `nif::glb`.
- `skeletons/<hash>.json` and `animations/<hash>.json` hold `Skeleton`
  and `Animation` as documents; the manifest lists them.
- `actors/<form>.json`, one per placed actor base after resolution:
  ```
  ActorDoc { form, editor_id, name, kind: npc | creature, female: bool,
             skeleton: hash, parts: [ { mesh: hash, path, role: body | outfit | head | hair,
                                        texture: path | null } ],   // a race's skin texture for the part
             animations: { stand: hash, walk: hash },
             tint: { hair: [u8; 3] | null }, appearance: String, health: i32, level: i32 }
  ```
  Templates are followed by their template flags, each group (traits,
  stats, model, base data, inventory) on its own; a leveled list resolves
  to its first entry at the lowest level, except an item list flagged
  "use all", which gives every entry, so the result never depends on a
  random draw. Files are `actors/<plugin>_<6 hex>.json`, listed in the
  manifest (`Mir::actors`, `Mir::actor`). The document also records the
  race, the template chains followed and its schema version (`mir::SCHEMA_VERSION`, 3 since 2026-11-24). `appearance` keys the sprite set: two actors with equal
  parts, texture overrides, tint and sex share one. Until the skeleton
  and animation stores exist the document names their source paths
  (`skeleton_path`, `stand_path`, `walk_path`); links that did not
  resolve are listed in its `notes`.
- A part list is: the race's body parts for the sex, minus the slots a
  worn armour covers; the worn armour's biped model for the sex; the
  race's default head; the hair model. No FaceGen morphs or textures.

`foundry-render`: unchanged. A frame is a `Scene` of posed meshes.

`emit`
```
pub struct CritterFrames { pub fps: u16, pub directions: [Vec<RenderedFrame>; 6] }
pub struct RenderedFrame { pub origin: [i32; 2], pub width: u32, pub height: u32,
                           pub pixels: Vec<u8>, pub travel: [f32; 2] }   // screen pixels walked in this frame
pub fn critter_frm(frames: &CritterFrames, action_frame: u16) -> Result<FrmSpec, EmitError>;
pub struct CritterList { .. }     // art/critters/critters.lst: from_base, append(stem) -> row, to_bytes
pub fn critter_prototype(common: ProtoCommon, spec: &CritterSpec) -> Result<Prototype, EmitError>;
```
`critter_frm` turns frame origins into one shift per direction and
frame-to-frame offsets, and spreads `travel` into whole pixels with the
remainder carried, so offsets sum exactly along the hex step.

As built (`emit::critter`, `emit::art`, `emit::proto`, `emit::map`):
- Frame `k`'s `travel` is the motion from frame `k - 1` to frame `k`;
  only its component along the facing's hex step counts. A cycle's total
  is rounded to whole units of the step (4 a hex on the diagonals, 32 a
  hex east and west), so cycles never drift. A stand has zero travel.
- Frames of one direction may differ in size; each is cropped and padded
  to the direction's common rectangle.
- `still_frm(&FrmSpec)` makes the placeholder: frame 0 of each direction.
- `CritterList` (stems of 1 to 8 ASCII letters, digits or `_`; a new
  line is `stem,row`), `critter_fid`, `CritterSpec::human`,
  `critter_prototype`, `CritterPlacement`, `critter_object`. Errors are
  `EmitError::Critter`.

`map-builder`: `BuildInput::critter_art` (`CritterArt { label, frms }`,
FRM bytes by two-letter suffix; `aa` and `ab` required) and
`BuildInput::critters` (`CritterPlacement { art, reference, name,
description, position, yaw, hit_points, female }`). Each critter goes to
the walkable hex nearest its source position, within 8 steps, that holds
no exit grid, door, entrance, return hex or other critter; one that
finds none is skipped and reported. `walk::Frame::orientation_of_yaw` and
`yaw_of_orientation` map between a reference's rotation and a hex
direction through the projection (a reference with no rotation faces
north, +Y; the walk animation travels along +Y, which confirms it).
`BuildOptions::critter_ai_packet` (`None`: chosen from the base `ai.txt`,
highest `min_hp`, then lowest `chance`, then lowest number) and
`critter_team` (99).

`mojave-convert`: a `critters` stage after `sprites`: actor documents to
sprite sets under `critters/<key>/` (cached by appearance, skeleton,
animations, calibration, palette, facing yaws), listed in
`critters/index.json`; the map stage places the humans in the window
that start enabled, by their own flag and their enable parent. Rendering
rules: gore-cap shapes are dropped; a rigid part (hair, a hat) follows
the head bone with the bone's rest rotation undone; hair is drawn with
the actor's hair colour multiplied in, and its under-a-hat copy dropped.
A figure is drawn 15 % wider than life about its upright axis and
lifted with a gamma of 0.78 before quantizing (owner playtest); the
walk's travel is not widened.
`critters --preview <editor id>` writes an enlarged picture for tuning.

### 10.4 Decisions for the first wave

- **Facing.** Direction `d` faces the ground direction whose screen step
  is hex step `d`, computed from the projection, never from a fixed angle.
- **Walk distance.** A walk frame's offset is the hex step scaled by the
  ground distance the animation travels in that frame over the ground
  length of that hex step. Feet may slide a little; steps always land.
- **Humans first.** Creatures use other skeletons and come after.
- **Exterior only.** The townsfolk in the window that start enabled, by
  their own flag and by their enable parent (the rule the slicer already
  applies to statics): at most eight of the ten, fewer if a parent keeps
  quest-only actors away. Interior residents wait for real interiors.
- **Stand and walk are real** (`aa` from the idle, `ab` from the walk).
  `ao`, `ba`, `bb`, `ch`, `cj`, `ra`, `rb`, `aq`, `ar` are placeholders
  made from the stand pose, so that a fight never breaks the engine;
  real ones come with the ragdoll work.
- **Stem** `mj` plus four base-36 digits of the sprite set's number.
- **Peaceful by construction:** no script, one shared team that is not
  the player's, an existing passive packet from the base `ai.txt`.

### 10.5 Open for the owner

1. Which base AI packet townsfolk use, and whether they share one team
   (attack one, all join) or stand apart.
2. Whether creatures (bighorners, ravens) belong in the first wave.
3. Whether townsfolk may be killed before real death art exists.

### 10.6 Critter stage options and `set.json` schema 2 (2026-11-23)

`critters::critters(inputs)` runs `critters_with(inputs,
&CritterOptions::for_run(inputs))`: the level, deaths, stride, figure look
and `--only` from the command line (`--animations`, `--deaths`,
`--stride`, `--figure-look`, `--only`), else the slice (`animations`,
`deaths`, `stride`, `figure_look`), else the first wave's: **minimal**, so
every existing slice keeps its critter art byte for byte
(`CritterOptions::default()`, the library's, stays at combat). The slice's
`[tone.figures]` is laid over the look's tone; a look other than the
default needs a level above minimal (refused by `Inputs::check_mode`
before extract). Above minimal, `CritterOptions::weapon_codes` comes from
`catalogue::weapon_codes`: each carried weapon's rebalance class gives
its code and modes, so the armed set drawn is the one the weapon
prototype's code asks for (a set without it carries the weapon:
`LootReport::skipped`). From Fallout 3 on the game is hashed into the
stage's key prefix (New Vegas keys unchanged). `CrittersSummary::lines`
are printed by `critters` and `all`.

`<key>/set.json` above minimal is schema 2 (`CritterSetMeta::schema`):
`appearance`, `label`, `stand_frames`, `walk_frames`, `walk_cycle_units`,
`animations` (every suffix written, weapon letters included: `read_set`
returns them all and the map stage ships them all), `missing_textures`,
`level`, `real` (suffix to `{ source, frames, fps, action_frame }`),
`placeholders` (stills), `weapons` (weapon letter to the model drawn in
hand) and `bytes`. A minimal set has no `schema` (it reads as 0) and the
first wave's fields only.

## 11. Simulation layer interfaces (Pass 2, settled 2026-11-24)

Task T0 of `docs/simulation-layer-spec.md` section 12. This section copies
the interfaces that spec settles; the design discussion stays there. **The
spec stays the design reference; where it differs from this section on an
interface, this section wins.** Nothing of the transpiler is built:
`crates/transpile` is a placeholder, and of the stage only the scaffold in
`crates/mojave-convert/src/sim.rs` exists (11.3). Spec section numbers are
written "spec 4.7" and so on; engine facts are the spec's E1 to E31.

### 11.1 The IR (`transpile::ir`, spec 3)

Plain data (`serde`, `BTreeMap`, sorted `Vec`), built from the MIR, checked,
then lowered. It never holds engine numbers; `emit` allocates those (11.4).
Order everywhere is `SimId` order. Texts are kept as decoded (Windows-1252)
and converted once in `emit` (spec 4.10); the converter's own labels
("[Continue]", the skill-check brackets) are `TextRole::Label` and are the
only text this repository writes.

Identifiers and text:

```rust
pub struct FormKey(pub String);           // "fallout3.esm:01a2b3", the MIR's form identity (section 2)

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone)]
pub struct SimId { pub kind: SimKind, pub form: FormKey, pub sub: u32 }
pub enum SimKind { Topic, Info, Quest, Stage, Objective, QuestVar, Global,
                   Script, ScriptVar, Package, Group, Terminal, MenuItem,
                   Note, Actor, Ref, Text }

pub struct TextId(pub u32);               // index into Module::texts
pub struct Text { pub owner: SimId, pub source: String, pub role: TextRole }
pub enum TextRole { Response, Prompt, TopicName, QuestName, Objective,
                    LogEntry, TerminalHeader, MenuItem, MenuResult,
                    NoteTitle, NoteBody, MessageBox, Label }
```

`sub` is a sub-index where one form makes several objects (a quest's stage
10, a terminal's menu item 2). Every lowered artifact (procedure name,
message id, global index) is derived from a `SimId`, so the report and the
debug overlay can name the source form of any number.

Expressions and conditions:

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
literal or a `GLOB`. `run_on` becomes the call's `on`: 0 subject, 1 target,
2 the condition's reference, 3 combat target, 4 linked reference. An
unsupported function is never silently true: it becomes `Cond::Fallback` with
a value chosen per function (spec 4.8, 4.9), and the comparison is evaluated
normally; every fallback writes one report line.

Dialogue:

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
    pub speakers: SpeakerSet,
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

`Skill` is in Fallout 2 order (SPEC-006 section 3.3) because it is a lowering
target; the source actor value maps to it in `dialogue::checks` (spec 4.7).

Scripts:

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

Quests:

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

Packages (type numbers as stored: 0 Find, 1 Follow, 2 Escort, 3 Eat, 4
Sleep, 5 Wander, 6 Travel, 7 Accompany, 8 UseItemAt, 9 Ambush, 10
FleeNotCombat, 12 Sandbox, 13 Patrol, 14 Guard, 15 Dialogue, 16 UseWeapon):

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

World state, terminals, notes, references:

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

/// Every placed reference something needs at run time (spec 5.8).
pub struct RefEntry { pub form: FormKey, pub editor_id: Option<String>,
                      pub base: FormKey, pub area: String, pub mobile: bool,
                      pub why: RefUses }      // bit set: script, condition, package,
                                              // quest target, enable parent, linked
```

The module and the two entry points:

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

transpile::build(mir: &mir::Mir, slice: &SimSlice, config: &SimConfig)
    -> Result<Module, TranspileError>
transpile::lower(&Module, &Placement, &BaseFiles) -> Result<Lowered, TranspileError>
```

**Helper types settled by wave S1** (2026-11-24; the code is
`crates/transpile/src/ir/` at `d3fa22f`; field lists below are copied from
it). The spec named these without defining them.

```rust
// ir/expr.rs
pub struct FuncId(&'static str);   // a function of obscript::functions::TABLE, by canonical name
                                   //   (as the public function list spells it: "GetStage").
                                   //   FuncId::named(&str) (case-insensitive) -> Option, .name(), .signature().
                                   //   Serialises as that name; reading a name the table lacks is an error.
pub enum UnOp  { Neg, Not }        // -x ; !x (1 when x is 0, else 0)
pub enum BinOp { Add, Sub, Mul, Div, Mod, And, Or, Cmp(CmpOp) }
                                   // Mod is not in the spec's grammar, kept because the source compiler accepts it;
                                   // And/Or give 1 or 0; Cmp gives 1 when it holds, else 0
pub enum CmpOp { Eq, Ne, Lt, Le, Gt, Ge }   // parse("==") / as_str() / holds(f64, f64) / negated()
// All three serialise snake_case. Func is Known(FuncId) | Unsupported { name: String }.

// ir/dialogue.rs
pub struct ActorValue(pub u32);    // a source actor value by its stored number (5..=11 SPECIAL, 16 Health,
                                   //   23 Karma, 32..=45 skills); serialised as the number (serde transparent)

// ir/world.rs
pub struct RefUses(pub u8);        // why a placed reference is needed: a bit set, serialised as the sorted
                                   //   list of the names of its bits
//   SCRIPT 0x01 "script" (its base has a script, or a script names it) | CONDITION 0x02 "condition"
//   | PACKAGE 0x04 "package" | QUEST_TARGET 0x08 "quest_target" | ENABLE_PARENT 0x10 "enable_parent"
//   | LINKED 0x20 "linked"; NONE = 0; contains / insert / is_empty / names. An unknown name is an error.
pub struct RefEntry { pub form: FormKey, pub editor_id: Option<String>, pub base: FormKey,
                      pub area: String /* the placement's map stem */, pub mobile: bool /* a critter */,
                      pub why: RefUses }

pub enum ActorKind { Npc, Creature, Leveled }  // NPC_ | CREA | LVLN or LVLC (a leveled list of actors); snake_case
pub struct AiData { pub aggression: u8, pub confidence: u8, pub energy: u8, pub responsibility: u8,
                    pub mood: u8, pub services: u32, pub assistance: u8 }   // AIDT after templates; Default is all 0
pub struct Actor {
    pub base: FormKey, pub editor_id: Option<String>, pub kind: ActorKind,
    pub script: Option<SimId>,                  // its object script (SimKind::Script)
    pub packages: Vec<SimId>,                   // in PKID order (SimKind::Package)
    pub factions: Vec<(FormKey, i8)>,           // faction and rank, in file order
    pub voice: Option<FormKey>, pub race: Option<FormKey>, pub class: Option<FormKey>,
    pub female: bool,
    pub essential: bool,                        // ignored by Pass 2 and reported (owner decision 12)
    pub ai: Option<AiData>,
    pub placements: Vec<FormKey>,               // its placed references in the slice, sorted
}

pub enum GlobalPurpose { QuestState, QuestVar, Glob, Group, Note, OnceFlag, Actor }
//   Declaration order is the allocation order of spec 10.2: by purpose, then by SimId.
//   Module::globals_in_allocation_order() returns them so.
//   QuestState: stage, flags, done and pending bits, objectives (spec 6.1); QuestVar: a quest variable (5.4);
//   Glob: a source GLOB; Group: an enable group's state (8.2); Note: a holodisk global (9.2);
//   OnceFlag: say-once and say-once-a-day flags, topics given, challenge outcomes (4.5);
//   Actor: per-actor globals (dead, faction overrides, kill counters, hoisted variables).
pub struct Global { pub id: SimId,           // the key in Module::globals: the source thing it stores
                    pub purpose: GlobalPurpose,
                    pub name: String,        // the <NAME> of MF_<KIND>_<NAME>: editor id uppercased, anything
                                             //   outside A-Z0-9_ replaced by _ (ir::global_name)
                    pub ty: VarTy,
                    pub initial: i32 }       // as stored: a float is times 1,000, rounded (ir::fixed_point)

// diag.rs
pub struct Span { pub line: u32, pub column: u32 }                 // from 1; the column counts characters
pub enum Severity { Error, Warning, Note }                         // lost something / approximated / only counted
pub enum Category { /* 40 kinds, grouped by Category::group(): "lex" (BadCharacter, UnterminatedString,
    BadNumber), "parse" (UnexpectedToken ... AssignAsCompare), "name" (UnresolvedName ... BadAssignment),
    "function" (UnknownFunction, DroppedFunction, ArgumentCount, ArgumentKind), "block" (UnsupportedBlock,
    EmptyBlock), "condition" (ConditionFallback, ConditionUnknownFunction, ConditionBadParameter,
    ConditionRunOn, ConditionComparison), "build" (MissingRecord, RefOutsideSlice, IgnoredFlag, QuestDepth) */ }
pub struct Diag { pub severity: Severity, pub category: Category,
                  pub owner: Option<SimId>,   // left out of the JSON when None
                  pub at: Option<Span>,       // left out when None
                  pub subject: String,        // the name, function or token concerned: the key the report counts by;
                                              //   left out when empty
                  pub message: String }
// Diag::{new, error, warning, note} build one; owned_by, at, about set the rest.
// Malformed source is never an error: it is a Diag.

// ir/module.rs
pub enum PlacedKind { Critter, Door, Container, Item, Terminal, Scenery }   // snake_case; as 11.2's `kind`
pub struct PlacedObject { pub map: String, pub map_number: u32, pub elevation: u8, pub hex: u32,
                          pub kind: PlacedKind, pub object_id: Option<u32>, pub pid: Option<u32>,
                          pub reference: Option<FormKey>, pub base: Option<FormKey>, pub hidden: bool }
pub struct Placement { pub objects: Vec<PlacedObject> }   // sorted (map, elevation, object id (none last), hex,
                                                          //   reference); sort(), of_reference(&FormKey)
pub struct SimSlice { pub name: String, pub placement: Placement }
pub struct SimConfig { pub sfall: bool,          // owner decision 6: false
                       pub hello_barks: bool,    // owner decision 14: false
                       pub quest_depth: u32 }    // quests followed beyond those the slice's code names; default 1
pub struct BaseFiles {                           // the base copies of spec 10.1, each file's text or None
    pub scripts_lst: Option<String>,   // scripts/scripts.lst
    pub vault13_gam: Option<String>,   // data/vault13.gam
    pub quests_txt: Option<String>,    // data/quests.txt
    pub holodisk_txt: Option<String>,  // data/holodisk.txt
    pub quests_msg: Option<String>,    // text/english/game/quests.msg
    pub pipboy_msg: Option<String>,    // text/english/game/pipboy.msg
    pub map_msg: Option<String> }      // text/english/game/map.msg
pub struct Lowered { pub files: BTreeMap<String, Vec<u8>>,   // layer path -> bytes, sorted
                     pub diagnostics: Vec<Diag> }

// error.rs
pub enum TranspileError {
    Mir(mir::MirError),                          // "reading the MIR"
    Query { query: &'static str, message: String },   // a records.sqlite table or column missing, or a row malformed
    Slice(String) }                              // inconsistent slice: a reference placed twice on one map, a
                                                 //   placement of another game
```

`SimConfig` derives `Default` and is read with `#[serde(default)]` (the
`[sim]` table of `foundry.toml`). `SimId` serialises as the text
`<kind>/<form>/<sub>`, for example `stage/fallout3.esm:01a2b3/10`, with the
kind as `SimKind::name()` (`topic`, `info`, `quest`, `stage`, `objective`,
`quest_var`, `global`, `script`, `script_var`, `package`, `group`,
`terminal`, `menu_item`, `note`, `actor`, `ref`, `text`), so that it can key
a JSON object; the form is split off at the last `/` when parsing, and a
text that does not read is `ir::BadSimId`. `SimKind` is declared in that
order, which is the order of `SimId` and of every numbering derived from it.

Differences from the text above and from the spec, as built:
`PlacedObject::hex` is a `u32` where 11.2's `hex` is an `i32` in the file
(the `sim` stage converts); `PlacedObject` also carries `map_number`,
`reference` and `base`, which the plain copy of "map, hex, elevation, PID,
object id, kind, hidden" did not list. `Placement` is the sorted list of
objects, and `SimSlice` is `{ name, placement }`. At `d3fa22f`
`transpile::build` and `transpile::build_module` exist; `transpile::lower`
does not yet (task T8), so `Lowered` and `BaseFiles` are defined and
unused. `ActorKind` has a third value, `Leveled` (a leveled list of actors).

### 11.2 The placement index (`map-placements.json`)

**Who writes it and who reads it.** The map stage writes it, on both paths
(`layer`, and `map::map` for one map), through
`mojave_convert::placements::write_for_stage`, beside `map-report.md` and not
inside the layer. The map stage also reads it, only to decide whether it can
reuse its output (`placements::is_current`: origin `stage`). The `sim` stage
reads it (`placements::read`); when the file is absent it makes what it can
from the written maps (`placements::reconstruct`, origin `reconstructed`) and
warns. The spec's `Placement` of `transpile::lower` is built from it
(11.1). Two builds from the same inputs give the same bytes: no time, host
or path is in the file.

**Schema, version 2** (`placements::SCHEMA_VERSION`, 2026-11-24). The text is
pretty-printed JSON with a final newline; an optional field is omitted, never
`null` (a test checks the text has no `null`); a reader treats an absent field
as its default. **A version-1 file is refused**: `PlacementIndex::from_text`
(and so `placements::read`) fails with "schema version 1 where this build
reads 2; run the map stage again", `is_current` is false for it (the map stage
does not reuse its output), and `sim` stops with that message. The fields
`key_pid` of a lock and `shape` and `radius` of a trigger were added to schema
2 as optional fields after it was first written; a reader of an earlier
schema-2 file sees them absent.

Top level, all fields always present except `triggers` and `notes`:

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | integer | 2 |
| `origin` | `"stage"` or `"reconstructed"` | who made it |
| `layer` | string | the layer's name, from its manifest |
| `game` | string | `mir::GameVariant::id` (`fallout3`); `sim` refuses a MIR of another game |
| `maps` | `[MapEntry]` | the layer's maps, sorted by (stem, file, number); includes the stub maps standing in for interiors outside the build (they place no reference; a note counts them) |
| `placed` | `[PlacedRef]` | references that became objects of a written map |
| `unplaced` | `[UnplacedRef]` | references of the maps' cells that are not on them |
| `triggers` | `[TriggerRef]` | the maps' trigger volumes; left out when empty |
| `notes` | `[string]` | how it was made; left out when empty |

`MapEntry`: `stem` (the file stem), `file` (`maps/<stem>.map`), `number` (the
map's number in the layer's manifest). `sim` checks that every entry is
registered in the manifest with that number.

`PlacedRef`:

| Field | Type | Optional | Meaning |
|---|---|---|---|
| `map` | string | no | the map's stem |
| `map_number` | integer | no | the map's number |
| `elevation` | integer 0 to 2 | no | |
| `hex` | integer 0 to 39,999 | no | the written object's hex when one was found, else the builder's |
| `kind` | `critter`, `door`, `container`, `item`, `terminal`, `scenery` | no | what it is (`PlacedKind`): a critter when the source is an actor or the written object is one; else by the base's signature (`NPC_`/`CREA` critter, `DOOR`, `CONT`, `TERM`, and `WEAP ARMO AMMO MISC ALCH BOOK KEYM NOTE CLOT INGR CHIP CMNY CCRD CDCK IMOD RCPE` item); else by the written object (a door, a critter, an item-category object with an inventory or room for one a container, any other item-category object an item); else `scenery` |
| `critter` | boolean | no | `kind == critter` |
| `object_id` | integer | left out when no object was found | the object's id in the written map |
| `pid` | integer | left out when no object was found | the object's PID |
| `pieces` | integer | left out when 1 | how many objects the reference became (a landmark is several pieces, ammunition several packs); the entry describes the first |
| `piece_ids` | `[integer]` | left out for one object | every object id of such a reference, the first included, in write order |
| `reference` | string | absent in a reconstruction (except a person named by `features/<stem>.json`) | the reference's form key; for a presence copy `<original>@<map stem>` |
| `editor_id` | string | left out when unknown | |
| `base` | string | left out when unknown | the base record's form key |
| `base_kind` | string | left out when unknown | the base record's signature (`DOOR`, `NPC_`) |
| `base_editor_id` | string | left out when unknown | |
| `enable_parent` | `{ reference, opposite }` | left out when none | the source's parent; `opposite`: the child is enabled when the parent is disabled |
| `group` | `{ root, opposite, initially_enabled }` | left out when the reference is in no enable group | spec 8.1: `root` is the group's root reference (chains flattened; a cycle is cut and counts as enabled); `opposite` (left out when false) the opposite flags multiplied along the chain to the root; `initially_enabled` whether it starts enabled: the root's own flag, flipped by `opposite` for a child (a child follows its parent, not its own flag) |
| `initially_disabled` | boolean | left out when false | the source's own flag (not the group's state) |
| `hidden` | boolean | left out when false | the written object carries the hidden flag (9.7) |
| `lock` | `{ locked, level, key, key_pid }` | left out when the source has no lock | `locked`: the written object carries the lock bit (the lock is written with the catalogue, `loot = true`); `level`: the source's level as stored, 0 very easy to 255 key only; `key`: the key's base record form key, left out when the lock names none; `key_pid`: the Fallout 2 prototype id of the converted key item, when the items catalogue made one for `key` (`placements::key_pids_of`, from `LootReport::items` of kind `key`), left out when the lock names no key or the key is not in the layer |
| `linked_ref` | string | left out when none | the source's `XLKR` |
| `team` | integer | critters only, left out when unknown | the critter's team in the written map |
| `ai_packet` | integer | critters only, left out when unknown | the critter's AI packet number |
| `hostile` | boolean | left out when false | the catalogue made the critter hostile (its packet is the hostile packet, `hostile_packet` of 9.7) |
| `presence_of` | string | left out unless a presence copy | the form key of the actor the copy copies (spec 7.6) |

A presence copy is a placed entry of its own, described as its original with the
original's `enable_parent`, `group`, `initially_disabled`, `lock` and
`linked_ref` left off.

`UnplacedRef`: `map`, `reference`, `kind` and `reason` always present;
`editor_id`, `base`, `base_kind`, `base_editor_id`, `enable_parent`, `group`,
`initially_disabled` (left out when false), `lock` (`locked` is always false;
`key` and `key_pid` as above), `linked_ref` and `detail` as for `PlacedRef`
(left out when empty). `reason` is one of `skipped` (the stage names a reason in
`detail`: a critter without art, a slicer skip, an enable-group child it could
not place; the first reason given for a reference is kept), `deleted` (the
winning plugin deleted it), `disabled` (the source starts it disabled and the
stage left it out: always so with `enable_groups` off; with it on, a member
the pass could not place is `skipped`) and
`not_placed` (it is in the map's cells and nowhere on the map: pruned by
declutter, off the level, outside the window's budget, or not something the
stage places, such as items with loot off); tested in that order. A reference
the builder placed is never listed here.

`TriggerRef` (a primitive on an activator, `mir::Reference::trigger_volume`; no
object stands for it, and `sim` attaches a spatial script at its hex, spec
5.7): `map`, `map_number`, `elevation`, `hex`, `reference` always present;
`shape`, `radius`, `editor_id`, `base`, `base_kind`, `base_editor_id`,
`enable_parent`, `group` and `linked_ref` as for `PlacedRef`, left out when
unknown or none. `hex` is the hex under the volume's centre. `elevation` is the
elevation of the run whose interior band holds the volume's height (an interior
with storeys), else 0; the one-map path always gives 0. `shape` is `"box"` or
`"sphere"` (`mir::PrimitiveShape::Box` or `Sphere`; a portal box or an unknown
shape gives neither shape nor radius). `radius` is the smallest hex radius
around `hex` that covers the volume's footprint, in hexes
(`placements::project_trigger`): for a box, the largest hex distance from the
centre's hex to the hex under any of the four corners of the box, taken with
the half-extents `bounds[0]` and `bounds[1]` scaled by the reference's scale and
turned by its yaw (a corner off the map is pulled in along its line to the
centre, to the map's edge, since nothing can stand beyond it); for a sphere, the largest to the four points `bounds[0]`
(scaled) reaches along the ground axes. The height is not used. `radius` is
left out when the reference has no box or sphere primitive. A trigger of a
deleted reference, or whose centre is off the map, is not listed. The radius is a covering circle, which is what a
spatial script's range is; the box's exact footprint (half-extents and yaw) is
not in the index, so a script is not told when the player is inside the circle
but outside a thin or turned box. The bounds are read as half-extents, which is
documented publicly and not verified here.

**Who is listed.** Critters, doors, containers, items and terminals, every
presence copy, and any other reference that is an enable parent or child
(a member of a group), that is linked by `XLKR` (either end), or that the
caller names (`referenced_of`: `mir::Records::referenced_refs()`, the legacy set
of section 2: a quest objective target, a script's reference list, a condition's
run-on reference, a linked reference, an enable parent or child, a package's
target or location; not the two reasons of revision 2). Plain scenery is not listed.

**Order.** `maps` by (stem, file, number). `placed` by (map, elevation, object
id with "none" last, hex, reference). `unplaced` by (map, reference).
`triggers` by (map, elevation, hex, reference). `notes` in the order made: for
each map, `<stem>: keys_missing N: locks name a key item the layer does not
hold` (the number of placed entries whose lock names a key with no `key_pid`;
none when N is 0), `<stem>: N references name an object id the written map does
not hold` and `<stem>: N references name an object whose hex or PID the written
map does not bear out`; then, for the whole file, `N stub map(s) place no
source reference`. A reconstructed index has one note saying so.

**How an object is matched.** Exactly, not by position: the builder returns
every object each placement and critter became (`map_builder::BuiltMap::objects`,
`Built::objects`: reference, elevation, object id, hex, PID, hidden), the first
object of a reference gives the entry's `object_id`, `pid` and `hex` (taken from
the written map, which must bear them out: the notes count an id the map does
not hold and one whose hex or PID differs), and the count gives `pieces`.
Nothing is guessed from distances; the `MOVED_RADIUS` search of the first
schema is gone.

**Reconstruction** (`placements::reconstruct`, origin `reconstructed`, used by
`sim` only when the file is absent). Made from the written maps alone: every
critter, door and container or item with an inventory (or room for one) becomes
a `PlacedRef` with `map`, `map_number`, `elevation`, `hex`, `object_id`, `pid`,
`kind`, `critter`, `hidden`, and for critters `team` and `ai_packet`; no
`reference`, bases, enable data, locks or links, except that a critter on a hex
`features/<stem>.json` lists a person for gets that person's `reference`. An
item lying on the ground with no inventory is listed as an item, and a
container with room as a container, both guesses. Terminals and scenery are not
listed; `unplaced` and `triggers` are empty.

**Differences from spec 10.4.** The spec lists "map, hex, elevation, PID,
object id and whether it is a critter" per placed reference. The index adds
`map_number`, `kind`, `pieces`, `piece_ids`, the source identity (`reference`,
`editor_id`, `base`, `base_kind`, `base_editor_id`), the enable-parent and group
fields, `hidden`, `lock`, `linked_ref`, `team`, `ai_packet`, `hostile`,
`presence_of`, and the `maps`, `unplaced`, `triggers` and `notes` lists. Spec 12
names `map-builder` as the writer in T7; the writer is `mojave-convert`
(`placements.rs`), which holds the join, with the builder supplying the exact
object map.

### 11.3 The `sim` stage (`mojave_convert::sim`, spec 10)

Command: `mojave-convert sim --out <dir> [--timings]` (`docs/running.md`,
"Sim"). It is a subcommand of its own and is not part of `all` yet (task T9).
It needs no game install, renders and builds no map, and everything it reads
is under `--out`.

**Inputs** (`SimInputs::load(out) -> Result<SimInputs, SimError>`, read once):
`game`, `layer`, `index` (the placement index, from the file or
reconstructed; placed references that name a reference but no base get the
base from the MIR), `from_file`, `warnings`, `logic` (what the MIR holds for
the placed bases: scripts, packages, dialogue infos, topics, quests,
terminals, notes, the references something points at, and the whole
extract's record counts), and `key_parts`. It fails when the MIR or the
layer is missing, the index is of another game, or an indexed map is not
registered in the layer's manifest.

**Run.** `run(&SimInputs) -> SimReport` is a pure function of its inputs;
`write_report(out, &report)` writes `<out>/sim-report.md`; `cli` loads, runs,
writes and prints. Today `run` only counts. Pass 2 replaces its body with the
build (`transpile::build` then `lower`); the spec does not fix `run`'s final
signature (today it cannot fail).

**Cache key** (spec 10.3; `SimInputs::cache_key`): `sim-1:` followed by the
content hash of the lines `<name>=<value>`, one per key part, in this order.
The version is `sim::SIM_VERSION`.

| Part | Value today |
|---|---|
| `sim` | `sim-1` |
| `transpiler` | **placeholder**: `none (Pass 2 scaffold)`; becomes the transpiler's version |
| `foundry.toml [sim]` | **placeholder**: `none (not read by the scaffold)`; the `[sim]` table is not read and not yet defined in `config.rs` |
| `game` | `GameVariant::id` |
| `slice` | hash of the manifest's slice |
| `plugins` | hash of the manifest's plugin list |
| `mir rows` | hash of the MIR rows the scaffold reads (scripts of placed bases with their source, packages, terminals, notes, dialogue infos and referrals); it must grow with whatever the build reads (spec: the canonical rows of every query) |
| `base file <path>` | hash, or `absent`, for each of `scripts/scripts.lst`, `data/vault13.gam`, `data/quests.txt`, `data/holodisk.txt`, `text/english/game/quests.msg`, `pipboy.msg`, `map.msg` (spec 10.1's base copies; a layer from the map stage has none yet) |
| `map stage key` | the text of `<out>/cache/map.key`, or `absent` (the stage keys stay in `<out>/cache` whatever `--cache-dir` says) |
| `map <file> (<number>)` | hash of each registered map's bytes, in file-name order (an addition to the spec, which names only the map stage's key and the index) |
| `placement index` | hash of the index's text as read (before the base fill-in) |

The spec's program cache (hash of a program's lowered IR to its `.int`
bytes) does not exist yet.

**Outputs.** Today: `<out>/sim-report.md` and the console lines (game,
layer, index source, placed and not placed per map and in all, the logic
counts, coverage, the cache key, the time); the layer is untouched. The
report has no time, host or path, so a second run on the same output is
byte-identical. Its sections today are `## Summary`, `## Warnings` (only
when there are any), `## Placed references`, `## Source logic for the placed
set`, `## Coverage`, `## Whole extract (for scale)` and `## Cache key`.

The spec's report (10.5) has these sections, each with compiled, folded,
dropped and fallback counts and the first owners by editor id: `## Summary`
(programs, rows, globals, messages, holodisks, quest rows), `## Dialogue`,
`## Conditions`, `## Scripts`, `## Quests`, `## Packages`, `## World state`,
`## Terminals and notes`, `## Locks`, `## Engine gaps hit`. Only `## Summary`
is common to both; the scaffold's other sections are replaced as the build
lands.

Files of the finished stage (spec 10.1; a base copy means the layer's
own file is read and ours appended):

| Path | Content | Base copy |
|---|---|---|
| `scripts/*.int` | every program | no |
| `scripts/scripts.lst` | base rows, then `<stem>.int ; <editor id> # local_vars=<n>` per program | yes |
| `text/english/dialog/<stem>.msg` | one per speaker, terminal and message-box program | no |
| `data/vault13.gam` | base globals, then ours (`MF_<KIND>_<NAME> := <value>;`) | yes |
| `data/quests.txt`, `data/holodisk.txt` | base rows, then ours | yes |
| `text/english/game/quests.msg`, `pipboy.msg`, `map.msg` | base entries, then ours | yes |
| `maps/*.map` | the map stage's maps with script records attached (the stage reads each built map, adds script records and the header's map script, writes it back) | no |
| `vault-mod.toml` | adds `id`, `api = 1`, the `[[global_scripts]]` and `[[hooks]]` rows | no |
| `sim-report.md` | the report, under `--out`, not in the layer | no |

**Determinism.** Every map, list and table is iterated in `SimId` or file
order; random draws happen only at run time, in the engine; no timestamp,
host name or path in any file; `--jobs` only parallelises per-program code
generation, collected in order.

### 11.4 Identifiers and numbering (spec 10.1, 10.2, 4.10, 9.2)

| What | Rule |
|---|---|
| Program stems (8 bytes) | `mm` + 6 base-36 digits (map scripts), `md` speaker programs, `mo` object programs, `mt` terminals, `mfquest` the dispatcher; numbered by `SimId` order within each prefix |
| Script rows | appended after the base list; row index = base count + i |
| Globals | base count G, then ours in `SimId` order by kind: quest state, quest variables, `GLOB`s, groups, notes, once-flags, actor globals; `data/vault13.gam` is a whole copy with ours appended, which keeps every base index |
| Dialogue message ids (per speaker program) | 1 to 99 converter labels; `10000 + 10 n + r` response r (0 to 8) of the speaker's INFO ordinal n; `10000 + 10 n + 9` its prompt; `900000 + t` topic t's name; 970 to 1099 left free (the engine's Ask About lookup) |
| Pip-Boy message ids | `map.msg`, `quests.msg`, `pipboy.msg` from 100000; holodisk title `100000 + k`, text window `200000 + 500 k` for note k |
| Teams | from 100, in faction `SimId` order |
| Reference slots | from 1, in `SimId` order; exported variable `r<k>` on its map |
| Text | written through `emit`'s MSG adapter: braces become parentheses, characters outside Windows-1252 become `?`, every change counted in the report |

A layer that grows `vault13.gam` changes the global count, so saves made
without it do not load with it and the other way round. Numbers are not kept
across rebuilds in Pass 2 (decision 9).

### 11.5 Owner decisions, locked 2026-11-23 (spec 13)

The owner accepted all 15 defaults as written. They are decisions for Pass 2.

| # | Decision |
|---|---|
| 1 | A speech challenge's outcome is remembered forever; re-asking repeats the branch |
| 2 | Fallout 3 and New Vegas skill thresholds compare 1:1 to Fallout 2 skill percentages; the percent formula uses the plugin's own settings |
| 3 | Hacking and lock picking: no minigame, automatic success at the required skill, XP from the plugin's settings |
| 4 | Notes are Pip-Boy holodisks with a message-line notice; voice notes as text |
| 5 | NPCs whose packages leave their map get presence copies; named actors only |
| 6 | sfall mode stays off (no `message_box`, plain Fallout 2 words) |
| 7 | Ship a whole `scripts.lst` now (needed for dialogue files), with a brief for gap S filed |
| 8 | Quest journal: objectives only |
| 9 | No `sim-ids.toml` allocation file in Pass 2 (on before long playtests) |
| 10 | `vault-vm` is a dev-dependency at the pinned revision, for tests only |
| 11 | Megaton townsfolk are one team, raiders one team |
| 12 | Essential NPCs: the flag is ignored and reported |
| 13 | Karma is a shadow global with no display until gap K |
| 14 | Hello barks are off in Pass 2 |
| 15 | New Vegas failed checks are shown with a "25/50" label and can be chosen (the source's failure line) |

### 11.6 Engine gaps each interface depends on (spec 11)

Each gap has a workaround, so none blocks the build except A, which `emit`
provides locally until the engine brief lands.

| Interface | Gap | Effect on the interface |
|---|---|---|
| `emit::int` and every `scripts/*.int` | A (INT assembler in `vault-vm`) | local adapter in `emit`; the assembler source text is the stable interface |
| `Terminal`, `MenuItem`, `Note` | T (terminal and document reader) | lowered to no-head dialogue and holodisks; the IR is the modal's model, so only `terminals::lower` changes |
| `scripts/scripts.lst`, `text/english/dialog/<stem>.msg` | S (dialogue files for `[[scripts]]` rows) | the layer ships a whole `scripts.lst` (decision 7) |
| `MoveTo` into another map; the map number and hex of the placement index | M (load a map at a hex, elevation and facing) | global plus `override_map_start` on entry |
| `Package`, `Location`, presence copies, `RefEntry::mobile` | P (a critter leaving a map) | copies sharing the actor's globals (decision 5) |
| `PackageKind::{Eat, Sleep}`, sandbox furniture | F (furniture actions) | stand in place |
| `Stmt` rewards of karma; karma conditions | K (a karma write word) | shadow global `G_KARMA` (decision 13) |
| `EnableGroup`, the `hidden` and `enable_parent` fields of the index | U (request a `map_update_p_proc` now) | a reference table in the calling program; changes can show up to 60 s late elsewhere |
| `Quest`, `Stage`, `LogEntry` | J (per-quest text page in the Pip-Boy) | objectives only; stage text is kept in the IR, not shown |
| `Objective::targets` | O (map or compass markers) | kept, not lowered |
| `SkillCheck::Threshold { shown_when_failed, partner }` | G (greyed options) | shown with a "25/50" label, selectable (decision 15) |
| `Response`, `NoteBody::Voice` | V (speech audio and lip sync) | text only; the response's sound field stays empty |
| `VarRef::Hoisted` | R (reading another instance's local) | hoisting to globals, allowed only for a persistent reference with one placement |

### 11.7 The `emit` interfaces the transpiler calls (2026-11-24)

Everything below is in `crates/emit` and re-exported at its root where noted;
`transpile` produces text and rows, `emit` writes bytes and checks them with
the engine's own parsers. Signatures are copied from `d3fa22f`.

**The INT assembler** (`emit::int`; the stand-in for the engine's own
assembler, gap A; the text format is `docs/vault-engine-briefs/int-assembler.md`
and the crate's module notes; verified against the engine's loader,
`vault_vm::IntProgram::parse_in`, by `crates/emit/tests/int_vm.rs`).

```rust
pub fn assemble(source: &str, words: &Words) -> Result<Vec<u8>, AsmError>;
pub fn procedure_names(image: &[u8]) -> Result<Vec<Vec<u8>>, AsmError>;

pub struct AsmError { pub line: usize, pub message: String }   // one-based line; 0 = the whole program (its size)
pub enum Game { Fallout1, Fallout2 }                           // host words 0x8000..=0x8155 / ..=0x8183
pub struct Words { pub game: Game, pub mods: bool, pub enhanced: bool }
impl Words {
    pub const FALLOUT1: Self;  pub const FALLOUT2: Self;       // mods and enhanced off
    pub const fn with_mods(self, mods: bool) -> Self;          // the manifest scripting words 0x8300..=0x8309
    pub const fn with_enhanced(self, enhanced: bool) -> Self;  // the enhanced range 0x8200..=0x82FF
    pub fn accepts(&self, word: u16) -> bool;                  // as the loader's WordSet does
    pub fn word_named(&self, name: &str) -> Option<u16>;
    pub fn name_of(&self, word: u16) -> Option<&'static str>;
}
```

`assemble` is deterministic (the bytes depend on the source text and the word
set alone), never panics, and names the source line of a refusal; a word outside
`words` is an error at its line. The sfall word set is not supported. Source
statements: `.procedure <name> [args=<n>] [flags=<f>] [deadline=<n>]
[condition=<label>] [body=<label>]`, `.import`, `.name`, `.literal <label>
"<text>"`, `.entry <label>` (required), `<label>:`, the pushes `push.i`,
`push.f`, `push.s`, `push.n`, `push.a`, `push.p`, `push.raw`, a mnemonic (a core
word of `emit::int::CORE_WORDS` or a host word of the set) or `op <word>`.
`flags=` is a number or names joined by `|`: `timed`, `conditional`, `import`,
`export`, `critical` (`emit::int::flags`); the conditional flag is the caller's
to set. The image is the 42-byte bootstrap, the procedure records, one
identifier record per distinct name, one literal record per distinct text, then
the code. Limits: `MAX_PROCEDURES` 65,536, `MAX_ARGUMENTS` 341,
`MAX_STRING_BYTES` 65,533, `MAX_IMAGE_BYTES` 64 MiB. `procedure_names` reads
only the procedure records and identifier table, for the layer's hook check
below.

**The appenders** (`emit::append`; each keeps the base file byte for byte,
adds rows after it, re-reads the result with the engine's own parser and refuses
a base the engine would refuse). Rows are `(key, row)` and are **numbered in
ascending key order whatever order they arrive in**, so a caller keying rows by
`SimId` gets the same numbers and bytes from any input order (spec 10.2); two
rows with one key are refused (`EmitError::Append`). Every function returns
`Appended<K, N> { bytes: Vec<u8>, numbers: Vec<(K, N)> }`, `numbers` in key
order.

| Function | File | Row | Number given |
|---|---|---|---|
| `append_messages<K>(base: &[u8], first_id: i32, rows: &[(K, Vec<u8>)]) -> Result<Appended<K, i32>, EmitError>` | a `.msg` | raw text bytes in the game's encoding, no sound | `first_id + rank`; an id the base holds is refused (a collision means two layers claim one range); at most `MSG_MAX_RECORDS` rows |
| `append_message_entries(base: &[u8], entries: &[MsgEntry]) -> Result<Vec<u8>, EmitError>` | a `.msg` | `MsgEntry { id: i32, sound: Vec<u8>, text: Vec<u8> }` with chosen ids | the entry's own id, in ascending order; a base id or a repeated id is refused. Text rules are the writer's: no `}`, LF, NUL or `0x1A`, at most 1,023 bytes |
| `append_globals<K>(base: &[u8], rows: &[(K, GamVar)]) -> Result<Appended<K, usize>, EmitError>` | `data/vault13.gam` | `GamVar { name: String, value: i32 }` | the global's index, from the base's count; the base must hold the `GAME_GLOBAL_VARS` marker unless empty (an empty base gets it) |
| `append_script_rows<K>(base: &[u8], rows: &[(K, ScriptRow)]) -> Result<Appended<K, u32>, EmitError>` | `scripts/scripts.lst` | `ScriptRow { stem: String, editor_id: Option<String>, local_vars: u32 }`, written as `<stem>.int ; <editor id> # local_vars=<n>` | the row index, from the base's row count; a stem the base or another row already uses (compared without case) is refused |
| `append_quests<K>(base: &[u8], rows: &[(K, QuestRow)]) -> Result<Appended<K, usize>, EmitError>` | `data/quests.txt` | `QuestRow { location, description, global, display, completed: i32 }`, one `a, b, c, d, e` line | the ordinal among the valid records |
| `append_holodisks<K>(base: &[u8], rows: &[(K, HolodiskRow)]) -> Result<Appended<K, usize>, EmitError>` | `data/holodisk.txt` | `HolodiskRow { global, title, text: i32 }`, one `a, b, c` line | the ordinal among the valid records |

A layer that grows `vault13.gam` changes the campaign's global count, so saves
made without it do not load with it (spec 10.2).

**Attaching scripts to a built map** (`emit::map`, re-exported at the crate
root: `attach_scripts`, `attach_scripts_to`, `ScriptAttachment`, `ScriptSite`,
`ObjectScript`).

```rust
pub enum ObjectScript { Item, Critter }     // SPEC-003 section 2.9 category 3 (every object but a critter) / 4
pub enum ScriptSite {
    Object  { id: u32, kind: ObjectScript },                 // the object with this unique id, top level or in an inventory
    Spatial { hex: i32, elevation: u8, radius: i32 } }       // category 1: fires on a hex within radius
pub struct ScriptAttachment { pub site: ScriptSite, pub script_index: u32 /* 0 ..= 0x00FF_FFFF */,
                              pub local_vars: u32 /* 0 ..= 1,000,000 */ }

pub fn attach_scripts_to(map: &mut MapFile, attachments: &[ScriptAttachment],
                         map_script: Option<u32>) -> Result<(), EmitError>;
pub fn attach_scripts(map: &[u8], prototypes: &impl PrototypeLookup,
                      attachments: &[ScriptAttachment], map_script: Option<u32>)
                      -> Result<Vec<u8>, EmitError>;
```

An object attachment adds one record to category 3 or 4 with the lowest free
serial from 1 in that category, and sets the object's script id and cached
script file; a spatial attachment adds a category-1 record. Records are
appended after the map's own in a fixed order (by object id; spatial ones by
place), so the order of `attachments` does not matter. **Re-runs change
nothing**: an object that already has a record of the same script index keeps
it, a spatial record with the same hex, elevation, radius and script is not
added again, and `attach_scripts` on its own output returns the same bytes. The
map script is a script index; the header stores it plus one (SPEC-003 3.3) and
no system-category record is written. A saved map keeps a record's local count;
any other map reads it back as 0 and the engine takes the count from the script
list row. `attach_scripts` parses the bytes as a Fallout 2 map with the given
prototypes (the records need their prototypes' sizes) and writes through
`MapFile::write`. **Refused, with the map left as it was** (`EmitError::Map`):
an id on no object (named), an id on several objects, a critter category on a
non-critter or the reverse, a script index over `0x00FF_FFFF`, over 1,000,000
local variables, a spatial hex outside 0 to 39,999, an elevation above 2, a
negative radius, a map script index over `0x00FF_FFFF`, and an object that
already has a different script.

**Registering scripts in the layer** (`emit::ModLayer`; `ModLayer::new(id, name,
version)` sets the manifest's `id` and `api`). The manifest is changed through
an amend that keeps the change only if the manifest still writes to TOML and
parses back and its registrations do not conflict (`ModRegistry::build`);
otherwise `EmitError::Manifest` and the layer is as it was. File names are
lowercased.

```rust
pub fn register_script(&mut self, file: &str, index: u32, local_vars: u32) -> Result<(), EmitError>;
pub fn add_global_script(&mut self, file: &str, repeat: u32, kind: u32) -> Result<(), EmitError>;
pub fn add_hook(&mut self, point: &str, file: &str, procedure: &str) -> Result<(), EmitError>;
```

- `register_script`: `scripts/<file>` becomes script-list row `index`
  (`[[scripts]]`, `local_vars` locals) without replacing `scripts.lst`.
- `add_global_script`: a `[[global_scripts]]` entry (SPEC-024a): its `start`
  runs once at setup and then every `repeat` steps (0: never again); its
  `map_enter_p_proc`, `map_exit_p_proc` and `map_update_p_proc` run at those
  events; `kind` is 0 to 3.
- `add_hook`: `procedure` of `scripts/<file>` on the engine hook `point`
  (`[[hooks]]`, a public hook name such as `HOOK_CALCAPCOST`).

**Write-time checks** (`ModLayer::write`, through `check_scripts`; a layer with
a violation is not written): every registered file (`[[scripts]]`,
`[[global_scripts]]`, `[[hooks]]`) is in the layer as `scripts/<file>`; no
`[[scripts]]` index is a row of the layer's own `scripts/scripts.lst` when it
has one (the engine refuses such an index, so a layer that ships a whole list
needs no `[[scripts]]` rows: the transpiler ships a whole list, decision 7); and
every hook's program is readable by `emit::int::procedure_names` and holds the
named procedure (compared without case). `write` replaces a previous output
directory and gives byte-identical folders for identical layers.
