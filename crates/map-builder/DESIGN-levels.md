# Levels: one layer, several maps, several elevations

Status: **built**, 2026-11-23 (steps 0 to 6 of (g); step 7, the wiring in
`mojave-convert`, is the lead's). Section 4 adds **exterior levels** (an
exterior map of up to three elevations, Megaton) and the other builder
fixes of the same evening; its wiring is the lead's too. Section 3 below is the as-built account:
the public API, what the caller supplies, and every place the code departs
from the plan in sections 1 and 2, which are kept as written (their
`file:line` references are to the code before the refactor). References
are `file:line` at the time of writing; `assemble.rs` is
`crates/map-builder/src/assemble.rs`, `vf/` is the pinned `vault-formats`
checkout (`target/cargo-home/git/checkouts/vault-engine-*/3adeac9/crates/vault-formats/src/`),
`core/` its sibling `vault-core/src/`.

## 0. What the formats and the engine allow

Checked in the pinned revision, not assumed.

- **Elevations.** A map holds three tile blocks and three object lists:
  `MapFile::tiles: [Option<Vec<TilePair>>; 3]` and
  `MapObjects::elevations: [Vec<MapObject>; 3]` (`vf/map.rs:222`). The
  header has `entry_elevation` (`vf/map.rs:46`, read as 0 to 2 at
  `vf/map.rs:390`). `MapFlags` has `OMIT_ELEVATION_0/1/2` (`vf/pro.rs:33`).
- **The writer decides omission itself.** `MapFile::write` clears bits
  `0xe` and sets `OMIT_ELEVATION_n` for every elevation whose tiles are all
  `(floor 1, roof 1)` and which has no saved object (`vf/map.rs:906-916`).
  So `emit::empty_map`'s `OMIT_ELEVATION_1 | OMIT_ELEVATION_2`
  (`crates/emit/src/map.rs:46`) never reaches the bytes on its own: an
  elevation with tiles or objects is written, a blank one is omitted.
- **An object's elevation word is its list slot.** The writer writes each
  object with the elevation of the list it is in (`vf/map.rs:973-976`,
  the word at `vf/map.rs:1057`), not `MapObject::elevation`. So the `0`
  passed to `plain_object` (`assemble.rs:1492`, `1512`), to
  `exit_object`'s `plain_object` (`assemble.rs:708`) and to
  `CritterRecord::from_prototype` (`assemble.rs:1554`) only has to agree
  with the slot; `emit::critter_object` refuses an elevation above 2
  (`crates/emit/src/map.rs:153`).
- **Exit grids carry map, hex, elevation and orientation**
  (`ObjectDetail::ExitGrid`, `vf/map.rs:151`, read at `vf/map.rs:789`).
  The engine checks the grids on the player's hex **and elevation**
  (`core/world.rs:2309-2316`), then `process_transition`
  (`core/host_skills.rs:1526`) calls `transition_to` (`:1573`), which
  `load_map`s the target (`:1581`) and places the player at the grid's hex
  and elevation only when the hex is in `1..40000` and the elevation in
  `0..=2` (`:1585`). A grid to **the same map number** therefore works for
  a change of elevation, at the price of a full map reload (the map is
  saved and read back). Hex 0 means "keep the entrance": never target it.
- **Stairs, ladders, elevators** are scenery subtypes with data
  (`SceneryData::Stairs { map, destination }`, `LadderUp/Down
  { destination }`, `vf/pro.rs:262-265`; map records `ObjectDetail::Stairs`
  / `Ladder`, `vf/map.rs:136-150`, packed destination `vf/pro.rs:739-750`).
  The engine parses and stores them (`core/world.rs:1823`); using a ladder
  up starts a climb sequence (`core/host.rs:2200-2215`,
  `core/host_motion.rs:1322`), and nothing moves the player on stairs
  without a script. This plan does not rely on them: **every link between
  elevations and maps is an exit grid.** A stairs object may still be drawn
  as plain scenery.
- **`data/maps.txt`** start points are `elev:E, tile_num:T` with `E` in
  0 to 2 (`vf/fallout2/maps.rs:11-15`, parsed at `:292-308`); records also
  carry per-elevation `can_rest` (`:29`). Map numbers go up to `MAX_MAP`
  999 (`:7`). Today the builder writes `elev:0` (`assemble.rs:1702`).
- **Navmesh links.** `NavTriangleDoc::links` names the neighbour across
  each edge (`crates/mir/src/model.rs:512-522`); `NavmeshDoc::door_links`
  names the triangle in front of each door (`model.rs:526-531`, `:556`).
  `TeleportDoc::position` is where the traveller arrives, in the
  destination cell's coordinates (`model.rs:411-422`; `exits.rs:246-253`
  already reads it that way for return hexes).
- **Object limit.** At most `MAX_OBJECTS` (100,000) top-level objects in
  the whole map, all elevations together (`vf/map.rs:10`, `:575-586`);
  `assemble.rs:1571` checks the one list today.

## 1. Where elevation 0 and "one map" are hard-coded today

| What | Where |
|---|---|
| exit grid object on elevation 0, destination elevation 0 | `assemble.rs:708`, `:714` |
| placement and blocker objects on elevation 0 | `assemble.rs:1492-1499`, `:1512-1519` |
| critters on elevation 0 | `assemble.rs:1554` |
| the map's tiles and objects in slot 0 | `assemble.rs:1598-1607`; stubs `:1651-1653` |
| header entrance elevation 0, omit 1 and 2 | `crates/emit/src/map.rs:42`, `:46` |
| `maps.txt` start point `elev:0` | `assemble.rs:1702` |
| one walkability grid, one `Frame`, one sampler | `assemble.rs:887-908`; `Built::walkable` (`lib.rs:540`) |
| exits only into interior cells, never between maps of the build | `exits.rs:236-245` |
| interior demo: exactly one map, zero exit grids | `assemble.rs:1739-1751` |
| one `Lists` per call (art, prototypes, messages) | `assemble.rs:162-401`, loaded at `:858` |
| numbering: exterior, then stubs at `exterior + 1 + index` | `assemble.rs:866-883`, `:1523-1530` |
| storeys merged by ramps | `crates/mir/src/storeys.rs:11-15`, `:88` |

## 2. Shape of the refactor

`build` today is one 950-line function (`assemble.rs:851-1805`) that
plans and writes in one pass. Split it into three layers:

1. **`LayerBuilder`** (new `layer.rs`): owns what every map of a layer
   shares, and is the only thing that touches `emit::ModLayer`.
2. **`MapPlan`** (new `plan.rs`): everything that reads inputs and decides,
   per map and per elevation, without writing: walkability, exits, islands
   and passages, entrance, critter spots, readability. Pure data.
3. **Writing** (what is left of `assemble.rs`): turns plans into art,
   prototypes, objects and map files through the `LayerBuilder`.

The order inside one build becomes: validate all inputs, plan every map
(phase A), resolve links between plans (phase B), check the whole layer's
budget, write (phase C), register and ship tables, verify. Planning before
any write is what lets doors on one map point at hexes decided on another.

### (a) `LayerBuilder`: what several maps share

```rust
pub(crate) struct LayerBuilder {
    layer: ModLayer,
    lists: Lists,                       // assemble.rs:162, moved as is
    names: BTreeMap<u8, ArtNames>,      // assemble.rs:1221 (one namer per list, layer-wide)
    tile_names: ArtNames,               // assemble.rs:1199
    art: BTreeMap<ArtKey, Fid>,         // layer-wide FRM dedupe (assemble.rs:404, :1135-1155)
    blocker: Option<(Fid, Prototype)>,  // one invisible blocker per layer (assemble.rs:1259, :1332)
    exit_grid: Option<Fid>,             // one marker and the misc padding (assemble.rs:1264, :1346-1369)
    numbers: Numbering,                 // first number, assigned numbers, stems
    maps: Vec<(String, u32, u8, i32)>,  // file, number, entrance elevation, entrance hex
    ship_maps_txt: bool,
}
```

- `Lists::load`, `add_art`, `add_prototype*`, `finish` move unchanged
  (`assemble.rs:183-401`). The critter list and its loading
  (`assemble.rs:1105-1131`, `Lists::load_critters` `:238`) move too: one
  `CritterList` per layer, sprite sets deduplicated by critter art index
  across maps of the same `LayerInput` (they share `critter_art`).
- **Numbering.** `Numbering::new(base maps.txt)` computes `first_number`
  as today (`assemble.rs:866-883`). Real maps take numbers in input order:
  a map's `map_number: Option<u32>` (`lib.rs:512`) is honoured, else the
  next free one. Stubs are numbered after **all** real maps, in (owning
  map order, door form key) order. For one map this is exactly
  `exterior, exterior + 1 + index` (`assemble.rs:1525-1528`).
- **Stems.** `map_stem` (`names.rs:45`) per map; stub stems keep
  `stub_stem(owner, index)` (`names.rs:59`) with the index counted per
  owning map, plus a layer-wide collision check (two maps whose stems share
  their first four letters would otherwise both make `salo_000`). A
  collision is an input error naming both maps; the caller renames.
- **`maps.txt` and the manifest.** `register_map` per map and the
  `maps.txt` text (`assemble.rs:1682-1716`) move into
  `LayerBuilder::finish`, with `elev:{entrance elevation}`.
- `LayerBuilder::finish` then ships the lists (`Lists::finish`), runs
  `verify_layer` once (`assemble.rs:1738`), and returns the layer.

### (b) Up to three elevations per map

```rust
pub struct ElevationInput<'a> {
    pub sampler: &'a mir::GroundSampler,  // GroundSampler::flat(storey floor) indoors
    pub ground: GroundTiles<'a>,
    pub art: &'a [ArtImage],              // this storey's slicer output
    pub placements: &'a [Placement],
    pub blocked_hexes: &'a [i32],
    pub interior: Option<InteriorBuild>,  // the storey's band; None = exterior rules
}
```

A map has 1 to 3 `ElevationInput`s, elevation `e` = index. They share the
map's `window`, `projection` and so one `Frame` (`walk.rs:23`): the
`Frame` maps source `(x, y)` to a hex independently of height
(`walk.rs:100-144`), and every storey is projected ground-relative to its
own floor (contracts 9, "Ground-relative height"), so **the hex under a
source point is the same on every elevation**. That is what makes stair
pairing a hex lookup.

Per elevation, `MapPlan` keeps what `build` keeps once today:

- `blocked`, `engine_blocked`, `open` (`assemble.rs:888-906`), the
  `verdicts` (`walk::verdicts` / `interior_verdicts`, `walk.rs:356`,
  `:502`, with that elevation's `InteriorBuild` band), `walkable`, the
  `Cover` (`assemble.rs:995`), passages (`passages::reconnect`,
  `assemble.rs:1014`) with that elevation's targets, the sealing border
  (`walk::sealing_border`, `walk.rs:667`), components
  (`component_report`, `assemble.rs:724`, over that elevation's objects)
  and readability (`readability::measure`).
- Every helper above already works on one 40,000-hex grid and needs no
  change; the refactor loops over elevations instead of calling once.
- **Critters** are assigned to an elevation by their reference's height
  in the cells, as `critter_levels` does for one band
  (`assemble.rs:591-608`): the elevation whose band holds `z`; outside
  every band, the nearest band with `clamp_levels`, else skipped "on
  another level" (`assemble.rs:611-630`). They are placed on that
  elevation's grid (`critters::place`, `critters.rs:206`) and written with
  `from_prototype(…, e, …)`.
- **Entrance.** One per map, on `MapInput::entrance_elevation` (default:
  the elevation holding `entrance_near`'s storey, else 0), chosen by
  `entrance` (`assemble.rs:783`) on that elevation's grid; written to
  `header.entry_elevation` and the `maps.txt` start point.
- **Writing.** Objects of elevation `e` go to `map.objects.elevations[e]`,
  tiles to `map.tiles[e]` (`Some` only for an elevation that exists),
  `map.objects.total` is the sum. Object ids stay unique across the whole
  map: one `take_id` counter (`assemble.rs:1483-1488`) runs over elevation
  0, then 1, then 2. `MAX_OBJECTS` is checked against the total.

### (c) Storeys

`mir::storeys` joins every triangle interval within `gap` (120 units,
`storeys.rs:23`) of the level so far (`storeys.rs:88`), so a stair or ramp
spanning two floors merges them into one level (`storeys.rs:11-15`). The
interior demo then walks only the band around one floor
(`InteriorBuild::band`, `lib.rs:418-421`). For several elevations the
navmesh must be cut into floors **and connectors**.

Proposed `walk::split_storeys(cells, &SplitOptions) -> StoreySplit` in
this crate (it reads only `CellDoc`, so it does not need `mir` to change;
it can move to `mir` later):

1. **Classify triangles by slope.** A triangle's steepness is its rise
   over its plan size (corner `z` spread over the plan-area-derived run).
   With hysteresis: below `flat_max` (0.25) it is **floor**, above
   `steep_min` (0.45) it is a **connector**, between the two it takes the
   class of the majority of its linked neighbours (`links`,
   `model.rs:519`), iterated in triangle order to a fixed point (bounded
   by the triangle count). Hysteresis keeps a gently sloping floor whole
   and a long ramp whole.
2. **Floors** are connected components of floor triangles through
   `links` whose mean heights stay within a band (`INTERIOR_BAND_BELOW` /
   `ABOVE`, `lib.rs:371-375`) of the component's area-weighted median;
   a component that drifts out of the band (a ramp modelled flat) is split
   at the band edge. Components below `min_storey_area` (a dais, a table
   top) join the floor they touch. Without links (synthetic data, links
   −1) the fallback is z-bands: `storeys_with_gap` over **floor triangles
   only**, then connectors attached by height overlap.
3. **Connectors** (stairs, ramps, ladders' navmesh) are connected
   components of connector triangles. Each records, per floor it touches
   (a linked floor triangle, or one whose edge lies within
   `navmesh_tolerance` in plan and `STEP_Z` in height), its **landing**:
   the touching edge's midpoint `(x, y, z)`.
4. **Elevations.** Floors sorted by floor `z`; up to three per map, the
   storey of the build's entrance first-listed among ties. A building with
   more than three storeys becomes several maps of the layer: storeys
   0 to 2 in `<stem>`, 3 to 5 in `<stem>b`, and so on; a connector between
   storeys 2 and 3 links the two maps instead of two elevations.
5. **Stairs become paired exit grids.** For a connector between floor A
   (elevation or map `a`) and floor B (`b`), with landings `LA` and `LB`:
   - on A, exit grids on the walkable hexes of A within one step of
     `hex_of_source(LA)` that lie on the connector's side (half plane
     towards `LB`, as `far_doorstep`'s outer side, `exits.rs:77-125`);
     their destination is B's **arrival hex**: the walkable hex of B
     nearest `hex_of_source(LB)` that holds no grid, at least 2 steps from
     B's grids (the return-hex rule, `exits.rs:296-332`), facing away from
     the stairs (`exits::orientation_towards`, `exits.rs:150`);
   - symmetrically on B.
   The connector's own hexes are not walkable on either elevation (they
   belong to no floor), so the grids sit on its threshold. Grids on A and
   arrival hexes on B are passage and critter targets on their elevation,
   as door grids are today (`passages::targets`, `passages.rs:195`).
   On the same map the grid's `map` is the map's own number.

### (d) Bidirectional teleport doors

Today `exits::plan` turns a door into an exit only when its teleport leads
to an interior cell (`exits.rs:236-245`), always into a stub, and takes the
return hex from the interior door's teleport (`exits.rs:246-253`).

New `exits::DoorIndex`, built in phase B from all plans:
`cell form -> (map index, storey -> elevation)` for every cell **a map of
the build was built from** (not merely present in `BuildInput::cells`: an
exterior build lists the interiors its doors lead to only for their return
positions, `lib.rs:526`), and `door form -> (map, elevation, hex)` for
every door placement.

For every door placement with a teleport (in map order, door form key
order):

- **Destination in the build**: the destination map is the one built from
  `teleport.cell`; the elevation is that cell's storey holding
  `teleport.position[2]` (or the storey of the partner door's
  `door_links` triangle, `model.rs:526-531`, when its height is out of
  every band); the arrival hex is the walkable hex of that elevation
  nearest `hex_of_source(teleport.position)`, within `RETURN_RADIUS`
  (`exits.rs:45`) steps of the partner door when its placement exists,
  never one of the partner's own grids, at least 2 steps from the partner
  door (the return-hex rule, `exits.rs:307-323`). The door's grids are
  found as today (walkable neighbours, else the far doorstep, `exits.rs:
  254-288`) and lead to `(map number, arrival hex, elevation, facing)`.
- **Back**: the partner door is handled by the same loop from its own
  teleport, so the link is bidirectional by construction, and each
  direction arrives where the source data puts the traveller. A partner
  that resolves to no walkable hex is a `SkippedTeleport` with the reason;
  its counterpart is still linked (one-way) and reported.
- **Destination outside the build**: a stub, exactly as today
  (`assemble.rs:1618-1676`), numbered after the real maps.
- **Interior demo check.** The test at `assemble.rs:1739-1751` becomes a
  per-build option, `LayerOptions::isolated: bool`: with it, the layer
  must hold exactly the given maps and no exit grid (today's interior
  demo, and the default for `build_map` with `BuildOptions::interior`); a
  linked layer is checked instead for every exit grid leading to a
  registered map, onto a walkable hex of the named elevation (a new
  `verify_layer` check over the written maps; today's check stops at "on
  the grid, elevation 0 to 2", `verify.rs:465-495`).
- Interior maps' doors are converted in a linked build (today they are
  only listed, `assemble.rs:558-581`, `:935-958`); in an isolated build
  they stay listed as now.

### (e) One art budget for the layer

The art lists are layer-wide (contracts 9, "What a mod layer can do"), so
the budget is too.

- The check `assemble.rs:1135-1195` moves to phase C's start and sums
  every map's distinct images (the `ArtKey` dedupe, `assemble.rs:404`,
  over all maps), all tile libraries, one blocker, one exit marker and the
  tile padding, against `lists.art_base` once. `FidBudget` names the list
  and the largest contributors across maps.
- `art_budget` (`budget.rs:107`) and `builder_entries` (`budget.rs:92`)
  stay as they are: they describe the layer's lists. The caller splits
  `available()` between its maps' slicer and terrain runs.
- **Ladder.** The budget ladder lives in the slicer (`SlicerConfig::ladder`,
  contracts 9.4); `map-builder` only checks. Proposed caller rule
  (`mojave-convert`): run `map_slicer::estimate` for every map and
  elevation, give each its share of `available()` in proportion to its
  estimate (largest remainder, ties by map order), then slice maps in
  order; whatever a map leaves unused (its demand after the ladder, less
  its share) is added to the next map's share. Deterministic, and a
  single-map layer gets exactly `available()` as today.
- **Tiles.** Today every library tile becomes an FRM with no dedupe
  (`assemble.rs:1197-1218`). In a layer, a tile identical to one an
  earlier map already wrote shares its FRM; within the first map nothing
  changes (the terrain library is deduplicated already, and leaving
  same-map tiles alone keeps one-map bytes identical).
- Prototypes stay per map (key `(map, art index, role, blocking)`,
  `assemble.rs:1294-1331`): prototype lists are not the constraint, and a
  layer-wide key would change the one-map output (the duplicate-wall test
  expects two wall prototypes, `tests/build.rs:62-65`).

### (f) Public API and source compatibility

```rust
pub struct MapInput<'a> {
    pub map_name: &'a str,
    pub map_number: Option<u32>,
    pub window: mir::MapWindow,
    pub projection: &'a foundry_render::Projection,
    pub elevations: Vec<ElevationInput<'a>>,      // 1 to 3
    pub entrance_elevation: Option<u8>,
    pub cells: &'a [mir::CellDoc],
    pub critters: &'a [CritterPlacement],         // assigned to elevations by height
    pub options: BuildOptions,                    // per map: slope, reconnect, entrance, interior, ...
}
pub struct LayerInput<'a> {
    pub layer_id: &'a str,
    pub layer_name: &'a str,
    pub base_files: &'a BTreeMap<String, Vec<u8>>,
    pub critter_art: &'a [CritterArt],
    pub maps: Vec<MapInput<'a>>,
    pub options: LayerOptions,  // standalone, write_maps_txt, isolated, link_doors, critter AI/team
}
pub struct BuiltMap { pub file: String, pub number: u32, pub walkable: [Option<Vec<bool>>; 3], pub report: BuildReport }
pub struct BuiltLayer { pub layer: emit::ModLayer, pub maps: Vec<BuiltMap>, pub stubs: Vec<(String, u32)>, pub verify: VerifyReport }
pub fn build_layer(input: &LayerInput) -> Result<BuiltLayer, BuildError>;
```

- **`build_map` stays** (`lib.rs:1039`), with `BuildInput`, `BuildOptions`
  and `Built` unchanged. It becomes a thin adapter: `LayerInput::single(
  &BuildInput)` (one map, one elevation from `sampler`, `ground`, `art`,
  `placements`, `blocked_hexes`, `options.interior`; `LayerOptions` from
  `standalone`, `write_maps_txt`, `critter_*`; `isolated =
  interior.is_some()`; `link_doors = false`), then `Built { layer,
  walkable: maps[0].walkable[0], report: maps[0].report }` with the
  layer-wide fields (`maps`, `verify`, `art_lists`, `art_entries`,
  `prototypes`, `maps_txt`, `first_map_number`) copied into that report.
  Every call in `mojave-convert` (`map.rs:1313`, the `BuildOptions`
  literals at `map.rs:508`, `:638` with `..Default::default()`, the
  `walk::*` helpers, `art_budget`) compiles unchanged.
- `BuildOptions` keeps its per-map meaning. Fields that are really
  layer-wide (`standalone`, `write_maps_txt`, `critter_ai_packet`,
  `critter_team`) are read from the first map by the adapter and are
  `LayerOptions` fields in `build_layer`; `build_layer` refuses maps that
  disagree with `LayerOptions` (no silent pick).
- **Byte identity of one-map outputs.** The single adapter must produce
  the same bytes. What could change them, and why it does not:
  object ids (one counter, elevation 0 first: same order), stub numbers
  and stems (same formula for one map), art names (one namer per list,
  same order: tiles, sprites, blocker, exit marker, `assemble.rs:1197-1270`),
  prototype order (per map, same loop), elevation words (slot 0 as now),
  omission bits (computed by the writer, `vf/map.rs:906-916`), `maps.txt`
  text (`elev:0` because the entrance elevation is 0), and the report
  (same fields, same order).

### (g) Order of work, tests, risks

Each step ends with `cargo test -p map-builder` green and the golden
digests untouched.

0. **Golden digests first.** `tests/golden.rs`: the FNV-1a digest of
   layer files and manifest (the one in `tests/critters.rs:23-41`) and of
   `format!("{:?}", report)` for every synthetic scene the tests build:
   `town` with and without base, the door scene (`tests/critters.rs:220`),
   exits, islands, standalone, the saloon interior (`tests/interior.rs`),
   populated critters. Recorded before any refactor; the existing
   `BEFORE_EMPTY` / `BEFORE_BASE` (`tests/critters.rs:43-44`) stay too.
   The lead re-checks the real Goodsprings layer against
   `out/baseline-goodsprings-2.sha` (handoff 9) at the end.
1. **Extract `LayerBuilder`** (lists, namers, numbering, registration,
   `maps.txt`, finish, verify) from `build`, mechanically. Golden green.
2. **Split plan from write**: `plan_map` returns a `MapPlan` for one
   elevation; `write_map` takes it and the `LayerBuilder`. Golden green.
   Unit test: planning twice gives equal plans (determinism without IO).
3. **Thread the elevation**: `ElevationPlan` per elevation; `exit_object`
   takes `(map, hex, elevation, facing)`; `plain_object(…, e, …)`,
   `from_prototype(…, e, …)`; `tiles[e]`, `elevations[e]`;
   `entry_elevation`; `elev:E`. Golden green. New tests: a synthetic
   interior with two floor rectangles 300 units apart, built as two
   elevations of one map: both tile blocks and object lists written, the
   omission bits as the writer computes them, ids unique across
   elevations, critters on the elevation of their height, `maps.txt`
   start point on the entrance elevation, `verify_layer` green.
4. **Several maps**: `build_layer` with an exterior and an interior map;
   tests: numbering (given and assigned, stubs after real maps), stems
   and stub-stem collisions refused, one FRM for an image both maps use,
   one blocker and one exit marker, the summed budget error naming both
   maps' contributors, `maps.txt` holding every record.
5. **Door links**: `DoorIndex`; tests with `common::teleport_door`
   (`tests/common/mod.rs:375`): an exterior door into an interior of the
   build and the interior door back get grids leading to each other's
   map, onto walkable arrival hexes not on the partner's grids and 2 or
   more steps from the partner door; a door to an interior outside the
   build keeps its stub; a partner with no walkable arrival is skipped
   with a reason and the other direction still links; `isolated` refuses
   any grid; the linked check refuses a grid onto a non-walkable hex.
6. **Storeys**: `split_storeys`; tests: two floors joined by a steep ramp
   are one `mir::storeys` level but two floors and one connector here; a
   gentle slope (between the two thresholds) stays one floor
   (hysteresis); a dais under `min_storey_area` joins its floor; without
   links the z-band fallback gives the same split; the ramp's landings
   become grid pairs on elevations 0 and 1 of the same map number, each
   arriving on a walkable hex of the other; four floors give two maps
   linked by one grid pair.
7. **Wiring** in `mojave-convert` (the lead): a `campaign` slice listing
   the exterior and its interiors; the per-map, per-storey slicer runs and
   the budget split of (e).

**Risks.**

- *Same-map transitions reload the map.* An elevation change through an
  exit grid saves and reloads the map (`core/host_skills.rs:1573-1585`).
  The one known save/load failure (autoplay `AuditMismatch` entering a
  `.SAV` on the Strip, handoff 9 "What does not" 4) is in that path, so
  multi-elevation maps will exercise it far more. Autoplay over a linked
  layer is the acceptance test.
- *Arriving on a grid.* Arrival hexes must never hold a grid of the same
  elevation; the engine clears a request made during the load
  (`core/host_skills.rs:1555-1563`), but a walker stepping off onto
  another grid would bounce. The return-hex rule's 2-step margin covers
  this; tests assert it.
- *Stairs geometry.* A connector's landing may fall on a blocked hex
  (a wall strip at the stair head); the arrival search widens to
  `RETURN_RADIUS`, and a failure is a reported skip, not an error.
  Real stairs in the source data may be modelled flat (no steep
  triangles): those floors stay merged, as today, and are reported.
- *Upper storeys draw over nothing.* An upper elevation's squares without
  floor are the blank tile (drawn black); the engine shows only the
  current elevation, so the floor below is not visible from above. That
  is expected Fallout behaviour, but worth an engine frame early.
- *Budget.* Every storey is its own slicer run with its own wall strips;
  a three-storey interior plus its exterior may not fit 4,096 walls even
  standalone. The split of (e) makes that a clear `FidBudget` per map
  before anything is written.
- *Names and numbers.* Stub stems collide across maps with a shared
  four-letter head; numbers must stay at or below 999 with `maps.txt`
  (`assemble.rs:1683-1688`). Both are input errors with the maps named.
- *Byte identity* is guarded by the golden digests (step 0); the most
  fragile parts are the art namers' order and the object-id order.

## 3. As built (2026-11-23)

### Byte identity

`tests/golden.rs` pins, for 18 single-map scenes (the town with and
without a base, standalone, numbered without `maps.txt`; the critter
town; the door scene; the doors scene with and without a base and
standalone; populated critters with and without a given packet and team;
the cut town with reconnecting on, off and with rings; the connected town
with two doors and a critter; the saloon interior plain, clamped with an
entrance point, and with an interactive door), the FNV-1a digest of the
layer (paths, files, manifest; the digest of `tests/critters.rs`) **and**
the digest of `format!("{:?}", report)`, recorded before the refactor and
unchanged after every step; `BEFORE_EMPTY` and `BEFORE_BASE` hold too.
`build_map` is the adapter of (f): one map, one elevation,
`link_doors = false`, `isolated = interior.is_some()`. To keep the report
digests, `BuildReport` gained **no field**: everything new is on
`BuiltMap` and `BuiltLayer`. The lead should still re-check the real
Goodsprings layer against `out/baseline-goodsprings-2.sha` (handoff 9).

### Public API (`src/lib.rs` unless noted)

- `ElevationInput<'a>` (:1097): `sampler`, `ground`, `art`, `placements`,
  `blocked_hexes`, `interior: Option<InteriorBuild>`.
- `MapInput<'a>` (:1114): `map_name`, `map_number`, `window`,
  `projection`, `elevations: Vec<ElevationInput>` (1 to 3),
  `entrance_elevation: Option<u8>`, `cells`, `critters`, `options:
  BuildOptions` (per map; `interior` must be `None`, and `standalone`,
  `write_maps_txt`, `critter_ai_packet`, `critter_team` must equal
  `LayerOptions`'; disagreement is an input error, not a silent pick).
- `LayerOptions` (:1147, `Default` :1168): `standalone`,
  `write_maps_txt`, `critter_ai_packet`, `critter_team`, `isolated`,
  `link_doors` (default `true`).
- `Landing` (:1185) `{ map, elevation, point }`, `StairLink` (:1196)
  `{ label, ends: [Landing; 2] }`.
- `LayerInput<'a>` (:1203): `layer_id`, `layer_name`, `base_files`,
  `critter_art`, `maps`, `stairs`, `options`.
- `LinkKind` (:1217), `LinkReport` (:1226), `ElevationReport` (:1251),
  `BuiltMap` (:1277: `file`, `number`, `entrance_elevation`, `walkable`
  per elevation, `elevations`, `links`, `critter_elevations`, `report`),
  `BuiltLayer` (:1302: `layer`, `maps`, `stubs`, `verify`, `links`).
- `pub fn build_layer(input: &LayerInput) -> Result<BuiltLayer,
  BuildError>` (:1314). `build_map` (:1089), `BuildInput`, `BuildOptions`
  and `Built` are unchanged.
- `verify.rs`: `LinkCheck` (:518), `pub fn verify_links(layer,
  base_files) -> Result<LinkCheck, BuildError>` (:549).
- `storeys.rs`: `SplitOptions` (:54), `Floor` (:92, `interior_build`
  :109), `StoreyLanding`, `Connector`, `StoreySplit` (:138), `pub fn
  split_storeys(cells, &SplitOptions) -> StoreySplit` (:334),
  `StoreyMap`, `StoreyStair`, `StoreyLayout` (:634, `place` :642,
  `stair_links(first_map)` :653), `pub fn layout(&StoreySplit, stem) ->
  Result<StoreyLayout, BuildError>` (:684).

### What the caller supplies

Per layer: the base files, every map's critter sprite sets (one
`critter_art` the maps' critters index), the stairs, the options.

Per map: name (number optional), window and projection (shared by its
elevations), the cells it is built from (for an exterior map its window's
exterior cells, plus the interiors its doors lead to for return positions;
for an interior map its interior cell), its critters (each assigned to the
elevation whose band holds its reference's height; give a map only the
critters of its own storeys), and its `BuildOptions`.

Per elevation, one storey's own run of the pipeline:

- `ground`: `map-terrain` once per storey, `build_ground_from(&MemoryLand::
  default(), ...)` with `blank_without_land` and **that storey's** floor
  bake as overlays; blank squares become `BLANK_SQUARE`. No change was
  needed in `map-terrain`; the test
  `each_interior_storey_builds_its_ground_from_its_own_overlays` shows two
  storeys' grounds are independent of each other and of build order.
- `art`, `placements`, `blocked_hexes`: `map-slicer` once per storey, its
  interior mode at that storey's `floor_z` with its own `cut_height` (the
  slicer takes one floor per run): references whose height belongs to
  another storey must not be sliced into this one.
- `interior`: `Floor::interior_build` (the storey's floor and band) or the
  caller's own `InteriorBuild`; `sampler` may be empty indoors.
- The art budget is the layer's. Proposed caller rule, unchanged from
  (e): estimate every map and elevation, give each its share of
  `available()`, pass on what a map leaves unused.

For a building of storeys: `split_storeys(&[cell])`, `layout(&split,
stem)`, one `MapInput` per `layout.maps[k]` with one `ElevationInput` per
floor in `floors` order, and `stairs = layout.stair_links(index of the
first of those maps in LayerInput::maps)`. `tests/layers.rs` (`tower`,
`tower_maps`) is a worked example.

### Departures from the plan

1. **`LayerBuilder`** (`src/layer.rs`) holds the lists, the namers, the
   layer-wide sprite dedupe, the tile dedupe, the map files and their
   registration and `maps.txt`. Numbering, stems, the budget check and
   verification are in `assemble::build_layer`, which orchestrates; the
   builder has no `finish` that verifies.
2. **Plan and write** are split into phases rather than a pure `MapPlan`:
   `plan::plan_map` (phase A: walkability, door grids), `links::resolve`
   (phase B: stairs grids, stub returns, door links, stairs links,
   arrivals), `plan::reconnect` and `plan::place_critters` (phase C),
   then writing in `assemble.rs`. The plans hold their `Cover` and
   protected placements, so they are not plain data; determinism is
   tested on whole layers built twice instead of plans planned twice.
3. **Write order** keeps one-map bytes: every map's and elevation's tiles
   first, then every sprite, then the blocker, the exit marker,
   prototypes per map (key `(elevation, art, role, blocking)`, since each
   elevation has its own art list), the blocker prototype, the misc
   padding, sprite sets, critter prototypes (maps, elevations, spots),
   then objects and files.
4. **Tiles** share an FRM with any earlier *elevation group* (map and
   elevation), not only earlier maps; within a group nothing changes.
5. **Budget labels.** In a layer of several maps the `FidBudget`
   contributors are the map stems (sprites) and `<stem> ground tiles`, so
   the error names each map's share; a single map's labels are unchanged.
6. **Reports.** Per-elevation results are `BuiltMap::elevations`;
   `BuildReport`'s per-elevation fields describe the entrance elevation,
   its `objects`, `critters`, `exits`, `skipped_teleports` the whole map,
   its layer-wide fields the layer. `critter_ai_packet` / `critter_team`
   are set on maps that placed critters. Notes of elevations above 0 are
   prefixed `elevation N:`. Input errors of a layer with more than one
   map or elevation name the map (and elevation); one-map messages are
   unchanged.
7. **Door destination.** The destination map and elevation are those of
   the **partner door's placement** when the partner's map is built from
   the destination cell; otherwise the band rule (the band holding the
   arrival point's height, else the nearest; band-less elevations hold
   every height). The `door_links` triangle fallback is not used.
8. **Arrival hex.** Candidates are walkable (before passages), not hex 0
   or the border, on no grid of that elevation, 2 to `RETURN_RADIUS`
   steps from the partner door (around the arrival point without one).
   Tiers: the component of the partner door's own grids, then the largest
   component, then any; nearest the source's arrival point. Passages then
   join it like any return hex. It faces away from the partner door (the
   teleport's yaw without one).
9. **Stub return hexes** in a layer avoid every grid of the elevation,
   stairs grids included (identical to today when there are none).
10. **Verification.** A new public `verify_links` (`VerifyReport` is
    unchanged): every grid leads to a map of the layer, onto an elevation
    that exists there, hex 1 to 39999, holding no exit grid and no object
    that stops movement; links without a return are listed, and
    `build_layer` refuses a layer whose one-way links differ from those it
    planned (and reported). It runs on `build_map` layers too, which pass.
11. **Isolated layers** also refuse stairs; an exterior map in one lists
    its doors as "not converted on an isolated layer". Without
    `link_doors` (and not isolated), exterior maps make stubs and interior
    maps list their doors. In a linked layer an interior map's doors
    without a placement on it are not listed (they belong to another
    storey's map).
12. **Storey split.** Slope is the plane's gradient, `|n_xy| / |n_z|`,
    not the corner spread over a run. Neighbours are triangles sharing an
    edge **by position** across every navmesh of the cell, not the stored
    `links` (which name triangles of one navmesh, or external-link
    indices on mesh edges); the z-band fallback applies only when no edge
    is shared at all. Landings are the mean of the connector corners lying
    on a floor triangle (plan tolerance, `step_z` 40 to its plane), not
    edge midpoints. Floor ties sort by larger area; the "entrance storey
    first among ties" rule is not implemented. `layout` is separate from
    `build_layer`: the layer does not spill storeys itself, the caller
    makes one `MapInput` per layout map; map names are the stem, then its
    first seven letters and `b`, `c`, and so on.
13. **Stairs grids** are the walkable hexes at the landing (its hex and
    neighbours, on the side towards the other landing first, then the
    nearest ring out to `door_reach`): the one nearest the landing and its
    neighbours among them, one to four grids, never a door's grid. The
    arrival is within `RETURN_RADIUS` of the other landing and 2 or more
    steps from that end's own grids.

### For other crates

- `mir`: `storeys::split_storeys` reads only `CellDoc` and could move to
  `mir::storeys` beside `navmesh_storeys` (floors and connectors with
  landings), leaving `map-builder` the layout and links.
- `emit`: nothing was needed. Maps of several elevations are `empty_map`
  plus `header.entry_elevation` and `tiles[e]`, the writer computing the
  omission bits. The local `exit_object` (an exit grid with its own and
  its destination elevation) and the one-pixel marker FRMs could move to
  `emit` when its multi-elevation constructor lands.
- `map-slicer`: one interior run per storey (above).
- `mojave-convert` (the lead): a campaign slice listing the exterior and
  its interiors; per map and storey, the slicer and terrain runs and the
  budget split; then `build_layer`.

### Risk for the lead to test in the engine

Every exit grid between elevations of one map leads to the map's own
number with a non-zero hex (asserted by the tests and by
`verify_links`), so the engine reloads the map and places the player.
That reload is the path where autoplay once reported an `AuditMismatch`
(handoff 9): autoplay over a linked layer, crossing stairs both ways, is
the acceptance test.

## 4. Exterior levels and the second pass of 2026-11-23

### What was wrong

Megaton (`MegatonWorld`) is a town of decks, catwalks and ramps over a
crater floor. One flat level drops every navmesh triangle whose corners
all stand more than `navmesh_max_height` (160) above the terrain (572 of
its 1,568 triangles) and merges what is left: the town splits into
islands, and the entrance goes to the largest component, which is the
slope-only ground of the cells without a navmesh outside the wall. On
this data "height above terrain" means little anyway: the land under two
of the nine cells lies some 10,000 units below the walkways, and the
heights of the rest climb continuously from 11,650 to 13,670.

### The rule (`src/levels.rs`)

Fallout 2 draws one elevation at a time and changes elevation only through
exit grids. Two surfaces may share an elevation exactly when they never lie
over one another (in plan, with more than a person's height between them);
everything else about height can be flattened as the one-level map already
flattens the terrain. So the split is by **overlap**, not by height bands
or height above terrain:

1. Triangles of the exterior cells, neighbours by shared edge position
   (`storeys` rule), slope classes with hysteresis (`storeys::classify`).
2. Regions: connected triangles of one class (floor: terrain, a deck, a
   roof; connector: a ramp, stairs). No band split of floors: an exterior
   floor follows its land.
3. Overlap: every triangle rasterized onto the hexes whose centre it holds
   (no tolerance, so a ramp beside the floor does not overlap it along the
   edge), with its plane's height there. Regions conflict on 2 or more
   common hexes with more than `clearance` (120) between them; the lower
   is below. A region over itself is cut by height bands.
4. Levels: the longest chain of regions below (0 when nothing walkable is
   under it, so a platform over solid ground stays on the ground level);
   then, largest first, a region moves to the level most of its
   neighbours are on when every conflict stays ordered; a chain longer
   than three goes to a conflict-free level (order aside), else one
   reached by moving the regions in its way, else is unplaced and
   reported.
5. Transitions: every contact (shared corners) between neighbouring
   triangles of different levels, grouped by region pair and distance
   (210 units), is a `LevelLink`; its ends lie 35 units into each side, so
   the grids stand on each elevation's last walkable hexes before the cut:
   at the **head** of a ramp that has nothing under it (the ramp stays on
   the ground level) or at its **foot** when it rises over other ground
   (the ramp goes up with its deck). `LevelSplit::stair_links(map)` gives
   them as `StairLink`s of the map with itself, which the layer turns into
   paired exit grids exactly as for interior storeys (section 3, 13).

Walkability of a level (`walk::verdicts` with `BuildOptions::level`, or
`ElevationInput::level` in a layer): only the level's triangles, at any
height (no "too high" filter), a covered hex walkable unless blocked (the
terrain slope under a deck plays no part); a hex in a cell without a
navmesh keeps the slope rule on level 0 and is `NoGround` above it;
passages never open off-navmesh hexes above level 0.

### API (`src/levels.rs`, `src/lib.rs`)

- `LevelOptions` (`flat_max` 0.25, `steep_min` 0.45, `clearance` 120,
  `min_conflict_hexes` 2, `tolerance` 24, `step_z` 40, `link_radius` 210,
  `nudge` 35, `max_levels` 3); `split_levels(cells, &frame, &sampler,
  &options) -> LevelSplit` (`frame` is `walk::Frame::new(&window,
  projection)`; the sampler only reports heights above terrain).
- `LevelSplit`: `regions` (`LevelRegion`: kind, level, chain, triangles,
  area, z range, mean z, plan box, hexes, height above terrain, split),
  `links` (`LevelLink`: label, regions, levels, points, contacts),
  `levels` (`LevelSummary` per level), `conflicts`, `conflict_pairs`,
  `needed_levels`, `unplaced`; `exterior_level(level) -> ExteriorLevel`,
  `level_count()`, `stair_links(map)`, `level_of_point(p)` (the surface
  under the point nearest in height, else within `NEAR_RADIUS` 160 in
  plan: doors standing in their walls, critters, arrival points),
  `reference_level(p)` (the highest surface at or below the point: where a
  placed reference is drawn), `surface_height(level, x, y)`, `summary()`.
- `ExteriorLevel { level, triangles: Arc<BTreeSet<(navmesh form, index)>> }`;
  `ElevationInput::level` (layers; not with `interior`) and
  `BuildOptions::level` (the walk functions; a map's must be `None` in a
  layer, an input error otherwise; `build_map` moves it onto its one
  elevation).
- Critters of a map with levels go to the elevation whose surface their
  reference stands on (`LevelGeometry`, the `level_of_point` rule over each
  elevation's triangles; unknown references to the entrance elevation). A
  door link arriving on such a map without a partner placement takes the
  elevation of its arrival point the same way.

### What the converter and the slicer must supply per elevation

For an exterior slice built as levels (a `[levels]` key or a per-slice
switch; the default stays one level and byte-identical):

1. **Split once per map.** `split_levels(&window_exterior_cells, &frame,
   &sampler, &LevelOptions::default())`; report `summary()`. With
   `level_count() == 1` build as today.
2. **One `ElevationInput` per level `k`** (`elevations[k]`, `level:
   Some(split.exterior_level(k))`, `interior: None`), all sharing the map's
   window, projection and `cells`:
   - `sampler`: the worldspace sampler on every elevation (only level 0
     reads its slope).
   - **References per level**: each reference goes to
     `split.reference_level(position).unwrap_or(0)` (the highest surface at
     or below its origin). Doors: `split.level_of_point(position)` (a door
     stands in its wall, beside its surface); a door's placement must be in
     that elevation's `placements` only: its exit grids, stub return and
     door links are then on that elevation by construction.
   - `art`, `placements`, `blocked_hexes`: one `map-slicer` run per level
     with that level's references only. Level 0 is today's exterior run
     (terrain ground, relief, landmarks). On levels 1 and 2 there is no
     terrain: references are anchored on the level's surface
     (`split.surface_height(k, x, y)` under their origin, instead of the
     terrain height of contracts 9 "Ground-relative height"), and the
     deck meshes themselves are baked as floor (the slicer's floor bake of
     interiors, cut at a height above the level's surface as an interior's
     `cut_height` is above its floor) rather than sliced as walls.
   - `SlicerConfig::open_hexes` of level `k`: `walk::standable_hexes(&frame,
     &sampler, &cells, &BuildOptions { level: Some(split.exterior_level(k)),
     ..options })`; `landmark_walkers` likewise from `walk::verdicts`.
   - `ground`: level 0 is today's `build_ground` over the terrain. Levels 1
     and 2: `build_ground_from(&MemoryLand::default(), ...)` with
     `blank_without_land` and that level's floor bake as overlays (the
     interior recipe of contracts 9.5); every `Ground::is_blank` square
     becomes `BLANK_SQUARE`. An upper level has floor only under its decks.
   - The art budget is the layer's (section 3): estimate every level and
     split `available()` among them.
3. **Stairs**: `LayerInput::stairs = split.stair_links(index of the map)`.
4. **Entrance**: `BuildOptions::entrance_near` = the arrival point of the
   teleport that leads into the worldspace (the gate door's partner in the
   parent worldspace: its `TeleportDoc::position`, which is in this
   worldspace's coordinates), else a slice key; `MapInput::entrance_elevation
   = split.level_of_point([x, y, z])`, else 0.
5. **Critters**: all of the map's; the builder assigns elevations.

**How the levels look.** The engine shows only the elevation the player
is on, so a walker on a catwalk does not see the crater floor below
(blank, drawn black where level 1 has no deck), and a walker on the floor
sees the decks above only if level 0's slicer run draws them (as it does
today: with every reference on level 0 they stand over the floor as
strips, cut by the see-through circle). Proposed: level 0 keeps drawing
the references of the upper levels (the town reads from the ground, as on
today's map); a later pass could put the decks in level 0's roof tiles,
which the engine hides over the player. Transitions reload the map (section
3, "Risk"): autoplay across the ramps both ways is the acceptance test.

### Megaton, measured (`examples/levels.rs`, 2026-11-23)

`cargo run --release -p map-builder --example levels -- out/fo3/megaton/mir
megatonworld -9048 -19624 -3055,-22100` (the window of the slice; the
entrance point is inside the gate, by the gate door `003b2c`):

| | level 0 | level 1 | level 2 |
|---|---|---|---|
| regions | 65 | 21 | 7 |
| triangles | 1,111 | 341 | 116 |
| area (units²) | 14,952,459 | 4,714,986 | 1,951,936 |
| hexes (centre inside; walkable with the 24-unit tolerance) | 3,031 (3,536) | 954 (1,184) | 396 (496) |
| z range | 11,652 to 13,669 | 11,683 to 13,674 | 11,802 to 13,038 |
| teleport doors (`level_of_point`) | 13: Moriarty's front, Water Processing, Jericho, Brass Lantern, Burke, Armory, Clinic, Common House, Lucas Simms (both), Player House, the two gate doors | 5: Craterside Supply, Men's Restroom, Lucy West, Billy Creel, Moriarty's second | 3: Nathan and Manya, Women's Restroom, Children of Atom |

- 93 regions (467 of 1,568 triangles are connectors), 38 overlapping
  pairs; the longest chain is 3, so **Megaton fits three elevations**
  with no region unplaced. With 1-hex conflicts it asks for four and one
  deck of 60 hexes (z 12,561 to 12,617, three surfaces under it) is left
  unplaced; clearance 80 or 200 also gives three.
- 33 transitions: 20 between levels 0 and 1, 11 between 0 and 2, 2
  between 1 and 2; most have 1 to 3 contact points (narrow ramp heads).
- Reach with nothing blocked, through the transitions, from the gate:
  level 0 all 3,536 navmesh hexes, level 1 1,134 of 1,184, level 2 all
  496: **99.0 % of the 5,216 navmesh hexes**. The 6,644 slope-only hexes
  of the cells without a navmesh outside the wall stay walkable and
  unreachable, so of every walkable hex 43.6 % (today 37.6 %, all of it
  outside the wall). Once the slicer blocks its strips and passages
  rejoin the islands, expect most of the navmesh share to hold; to be
  measured on the written layer with `probe walk`.

### The other fixes

- **Entrance target** (`BuildOptions::entrance_near`, default `None`):
  `plan::reconnect` and `plan::entrance` (crate notes "Entrance").
- **Coverage rule** (`BuildOptions::navmesh_coverage`, `walk::NavmeshCoverage`
  `Tolerance` (default) / `Centre` / `Share(f)`, the hex's own screen
  pixels sampled every 4 pixels; `examples/coverage.rs`). Measured with
  each written map's blockers:

  | | tolerance (default) | centre inside | share 0.25 | share 0.5 | share 0.75 |
  |---|---|---|---|---|---|
  | Prospector Saloon, hexes covered | 244 | 181 | 217 | 185 | 144 |
  | reachable from the entrance | 197 | 153 | 176 | 158 | 119 |
  | doors with a reachable doorstep (of 6) | 6 | 5 | 5 | 5 | 3 |
  | Super-Duper Mart, hexes covered | 2,996 | 2,585 | 2,886 | 2,621 | 2,330 |
  | reachable | 2,751 | 2,427 | 2,675 | 2,454 | 2,130 |
  | doors (of 21) | 21 | 21 | 21 | 21 | 9 |
  | Moriarty's Saloon, hexes covered | 155 | 112 | 140 | 117 | 92 |
  | reachable | 110 | 86 | 105 | 90 | 41 |
  | doors (of 3) | 3 | 3 | 3 | 3 | 1 |

  Every rule keeps each room one navmesh component up to 0.5; at 0.75
  aisles and doorways break. The saloon door on hex 18091 loses its
  doorstep under every stricter rule (its doorway is covered only within
  the tolerance); the builder then finds a doorstep further out
  (`door_reach`) when it converts the door. Recommended for interiors with
  the slicer's footprints: `Share(0.5)`. These counts use the blockers the
  tolerance rule's run wrote; a rerun with the slicer's own open hexes will
  differ.
- **Footprints** confirmed: `tests/loot.rs` asserts that no footprint hex
  is walkable, holds the entrance, a critter, an arrival, a link's
  destination or a passage.
- **Readability** (`cover::Cover::shares`, `SightRule`,
  `BuildOptions::readability_sight` and `entrance_sight`, both
  `AnyPixel` by default: the golden report digests pin the old counts).
  The any-pixel rule counts a few pixels as covering: on the people's
  hexes of the readable saloon, 1 to 6 % of the box under stool and table
  tops one to four rows south, over the figure's feet and legs (box rows
  46 to 71 of 72). In `out/interiors/saloon-readable/shots/saloon-00-classic.png`
  seven of the eight people stand on hexes it calls `Covered` and all of
  them are in plain view (the eighth, the player, is `Behind`); the share
  rule calls six of them clear and one covered (the Settler at the bar, 26
  %, on the frame's left edge, which the frame cannot confirm). With
  `SightRule::VISIBLE` (covered = more than a quarter of the 32 x 72 box
  under art drawn after the figure; art behind never hides), on the
  walkable hexes of the written maps (`examples/sight.rs`): Prospector
  Saloon 68.5 % covered (plain) and 40.7 % (readable), against 97.6 and
  94.3 % by any pixel; Super-Duper Mart 54.7 and 36.4 % (any pixel
  71.8 % readable); Moriarty's 52.7 and 40.9 % (85.3 %). The slicer's
  floor metric says 29.7, 27.9 and 37.1 % hidden after declutter: same
  direction, the box being larger than a figure. As an entrance rule it
  moves one golden scene's entrance (`saloon-clamped`, hex 20698 to
  20700; `tests/sight_rules.rs`).
- **Linked interiors**: `BuildReport::interior_summary` says "interior map
  ... one of N maps in the layer" instead of "one map, no exits" when the
  layer holds more than its map, and lists doors not linked;
  `BuiltMap::interior_summary` adds `BuiltMap::link_lines`.
- **Critter components**: `ComponentReport::critter_reach`,
  `stranded_critters`, `BuildReport::critters_summary`; interior summary
  lines name each critter's component and whether the entrance reaches
  it.
- **`map-terrain` tone**: `TerrainOptions::tone: Option<foundry_render::Tone>`
  (default `None`; `IDENTITY` changes no byte), on the land colour before
  the overlays, which the slicer tones itself.

### Not done

- The converter wiring of levels (the per-level slicer runs, references
  per level, surface-anchored slicing above level 0, the budget split).
- Upper-level decks in level 0's roof tiles.
- An option to drop slope-only hexes of cells without a navmesh when the
  window has a navmesh elsewhere (Megaton's unreachable outside ground).
- A doorstep exemption for the stricter coverage rules.
