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
with the Capital Wasteland's. Nothing else differs.

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
  (`out/baseline-goodsprings-2.sha`).
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
  | 13, 15, 19 | `an`, `ap`, `at` | dodge, hit from behind, run | engine falls back |

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
  race, the template chains followed and its schema version (2). `appearance` keys the sprite set: two actors with equal
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

