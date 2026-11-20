# Pass 1 handoff: M0 to M3

Written 2026-11-20. Pinned `vault-formats` revision: `3adeac9`.

## 1. What was done

All four milestones passed their gates. The final workspace gate:

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo check --workspace --all-targets` | clean |
| `cargo test --workspace` | 534 passed, 0 failed, 1 ignored (needs a real palette) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |

Tests per crate: `fnv-bsa` 30, `dds` 28, `fnv-esm` 72, `nif` 61, `mir` 38,
`foundry-render` 38, `quantize` 24, `emit` 49, `foundry-gallery` 28,
`map-terrain` 20, `map-slicer` 32, `map-builder` 21, `mojave-convert` 10.
About 59,600 lines of Rust in 15 crates.

Each milestone was also run against a real New Vegas and Fallout 2 install,
which the original plan left for later:

| Stage | Real-data result |
|---|---|
| Archives | 41,662 entries read from three archives, 0 name-hash mismatches |
| Textures | 22,054 of 22,054 decode |
| Meshes | 14,853 of 14,881 parse and extract; the other 28 are another file version, correctly refused |
| Plugin | 465,016 records walked, 0 decode errors, 2,576 scripts all with source text |
| Extract | Goodsprings slice (64 cells, 11 interiors, 7,145 references) in 7 s; byte-identical across runs and job counts |
| Sprites | 307 of 324 prop meshes become FRM files in 5 s |
| Map | `goodspri.map` plus 15 stub maps (one per door), 6,827 layer files; fits the art budget (walls 2,268 of 2,398 free, scenery 603 of 2,225) |

### Agent topology

At most four agents ran at once, each owning whole crates.

| Wave | Agents | Crates |
|---|---|---|
| M0 | lead only | workspace, docs, contracts, scaffolds |
| M1a | 3 | `fnv-bsa` + `dds`; `fnv-esm`; `nif` |
| M1a calibration | 2 (resumed) | `fnv-esm`, `nif`, against real files |
| M1b | 1 | `mir` |
| M2 | 4 | `foundry-render`; `quantize`; `emit` + briefs; `foundry-gallery` |
| M3 | 3 | `map-terrain`; `map-slicer`; `map-builder` |

The lead wrote `mojave-convert` (the command line, the stage glue, the
probes, the end-to-end tests), `mir::ground`, the contracts, and ran every
gate and real-data run. `map-builder` was split from the planned single
crate into three so that no two agents shared a crate.

### Deferred, and known limits

Nothing is a stub. These are real limits of what was built:

- **Checked in the engine mostly headless.** Section 5 has what those
  runs found. The owner has since opened it in a window (2026-11-20, a
  silent debug build); their notes on how it plays are not recorded here.
- **No world-map entry.** The map is reachable by a start-map setting or
  `load_map`, not by travel. Exits into interior stubs and back are wired.
- **Rocks and cliffs are painted on the ground.** They block movement but
  never hide a character standing behind them.
- **Blocking uses bounding boxes**, except within 6 hexes of a door that
  leads somewhere, where the navmesh decides (section 5). A canopy away
  from any door is still blocked.
- **Trees.** 710 references in the slice use SpeedTree files, which nothing
  converts; they are absent from the map.
- **No roofs, scripts, music, critters or item inventory art.**
- **2,964 mesh shapes have no texture** (unlit, water, sky, grass, distant
  terrain) and render flat grey.
- **Calibration is partly settled**: the owner judged the scale right in
  play (2026-11-20), so the earlier "15 to 20 % large" estimate is
  withdrawn; relief shading darkens west-facing slopes heavily;
  the land-texture repeat distance (512 units) is assumed; the Euler axis
  order for placed references is settled only for rotation about Z; blues
  quantize poorly because the Fallout 2 palette has few.
- **Sprites are not shared across rotations**, so 105 bushes cost 105
  scenery entries. There is headroom, but it is wasteful.
- **The default render path is the CPU one**, for identical bytes on every
  machine. The `wgpu` path exists, is tested against it, and is opt-in.
- **The floor tile library is capped by the base game's list**: 977 tiles
  for this map, because a stock install leaves about 985 free.

## 2. Verification against `vault-formats` at `3adeac9`

Round trips checked in `crates/emit`:

1. `FrmArchive::parse` of every written FRM: header, six shifts, every
   frame; 1 and 6 directions, shared blocks, zero-size and extreme frames.
2. `sprite_shift` against `FrmFrame::world_rect` at `HexGrid::hex_center`.
3. `LstTable::parse` for art, script, prototype, raw and blank lines; empty
   and 4,096-entry lists.
4. `MsgCatalog::parse_strict` and `parse`.
5. `GamFile::parse`, both sections.
6. `Prototype::parse` and `parse_for(Fallout2)` for six constructed kinds.
7. `LocalPrototypeCatalog` resolves appended prototype ids.
8. `MapFile::parse_for` with re-written bytes identical.
9. The written folder: `ModManifest::parse`, `LayeredVfs`, `ModRegistry`.

`map_builder::verify_layer`, run on the layer as read back from disk:
manifest and map registrations; every FRM; every list (art lists at most
4,096 entries); every message file; every prototype (parses, re-writes to
the same bytes, resolves through the catalog); every map (version 20, no
diagnostics, every object FID and floor tile resolves through
`ArtCatalog::load`, every exit leads to a registered map); `maps.txt`; and
no stray file.

The end-to-end test (`crates/mojave-convert/tests/end_to_end.rs`) repeats
the parsing independently of the builder, through the library and through
the binary.

A test in `crates/emit/tests/vault_formats_exports.rs` pins the
`vault-formats` surface this pipeline uses and the three projection
vectors `(+48, -12)`, `(+32, +24)`, `(0, -96)`.

### Owner briefs for `vault-engine`

In `docs/vault-engine-briefs/`, written only from `vault-engine`'s own
source and specs, with no mention of this pipeline:

| File | Asks for |
|---|---|
| `frm-writer.md` | `write_frm` and `FrmArchive::write` |
| `lst-writer.md` | a list writer and an appending editor that enforces 4,096 art entries |
| `msg-writer.md` | a message-file writer that refuses what the format cannot carry, and can append |
| `gam-writer.md` | `GamFile::write` |
| `int-assembler.md` | a script assembler with a text syntax; ten open questions for the owner |

One thing the briefs do not cover and the pipeline wants most: **list
merging in mod layers.** Today a layer file replaces the base file whole,
so the layer carries full copies of the user's base lists and two such
mods override each other.

## 3. Running it

```
cargo build --release -p mojave-convert
```

```
target/release/mojave-convert all --fnv "<New Vegas install>" --f2 "<Fallout 2 install>" --out out/goodsprings
```

Then read `out/goodsprings/mir/report.md` and `out/goodsprings/map-report.md`,
and look at `out/goodsprings/gallery/` (`map-goodspri.png` is the map
composite; `sprites-NN.png` are the contact sheets). `out/` is gitignored.

Useful variations: `gallery --filter goodsprings` for one focused sheet;
`--config foundry.toml` to try a calibration; `--outline` for edged
sprites; `--backend gpu`.

## 4. Backlog for Pass 2

In dependency order. Items in one group can run in parallel on separate
agents; a group starts when the one before it is gated.

**A. Fix what the engine run found (section 5).** The headless run is
done; these came out of it, most valuable first:
1. Done (section 5): porch blocking near doors; all 15 doors convert.
   Opening the navmesh everywhere needs 3,195 wall entries against 2,398
   free, so the rest of the over-blocking waits for cheaper strips
   (rotation-shared art, B2) or collision-mesh blocking (B3).
2. Done (section 5): one stub map per door, so leaving comes out at the
   door that was used. When interiors become real maps this turns into
   one map per interior with an arrival hex per door.
3. Keep exit grids off through routes, or put the arrival hex on the
   near side of them. Crate: `map-builder`.
4. Report walkable islands in `map-report.md` and either join or block
   them: 1,303 walkable hexes cannot be reached from the entry. `probe
   walk` already computes this; move it into the build.
5. Done: the owner played it in a window (section 6).

**B. Real-data calibration (three agents).**
1. Light: tune the key light and `relief` against original art using the
   contact sheets; settle the Euler axis order on tilted references. Scale
   is settled (section 6). Crates: `foundry-render`, `map-terrain`.
2. Mesh gaps: textures for unlit and special shaders, SpeedTree billboards
   for the 710 trees, rotation-bucketed sprite sharing. Crates: `nif`,
   `map-slicer`.
3. Map quality: collision-mesh blocking instead of bounding boxes, roofs
   as a roof layer, occluding rocks, tile-edge seams, a second map window
   and world-map entry. Crates: `map-slicer`, `map-builder`, `map-terrain`.

**C. M4, characters.** The contract is `docs/contracts.md` section 10:
what the engine requires of a critter, what the extraction holds, the
interfaces per crate, the first-wave decisions and three questions for
the owner.
1. Wave M4a, one crate each: `nif` (skeleton, skin weights, `.kf`
   animation, posing and CPU skinning); `mir` (actor documents: templates,
   leveled lists, race body parts, outfits, skeleton and animation
   stores); `emit` (critter frame sets, list, prototypes).
2. Wave M4b: `map-builder` (critter placements), `mojave-convert` (the
   `critters` stage), then the real slice and the engine check.
3. Later: creatures, FaceGen heads (a research task first, under the
   `fnv_decomp` read-only policy), ragdoll deaths with `rapier3d` in
   `crates/ragdoll`, interior residents.

**D. M5, scripts (three agents, after the INT assembler lands upstream).**
1. Upstream: `vault-engine` implements `int-assembler.md` and answers its
   ten open questions. Then swap `emit`'s adapters for the upstream writers.
2. Transpiler front end in `crates/transpile`: source text to an
   intermediate form. The report ranks 479 command identifiers by use and
   80 condition functions by index; cover the top of both first. `Begin
   GameMode` blocks and package schedules map to the engine's timed events.
3. Back end and the four Goodsprings quests, with dialogue to `MSG`; depends
   on 1 and 2.

## 5. In-engine check (2026-11-20)

The Goodsprings layer was loaded in `vault-engine` at its committed head
(`df2bf4c`, 14 commits after the pinned `3adeac9`), built from a clean
export outside the engine's checkout, over the real Fallout 2 archives.
The engine's source was not touched. All of it ran headless: no window,
no GPU, no audio.

What was run:

- **Autoplay.** `--autoplay-headless --preview-map maps/goodspri.map` with
  the layer as the mod list: 2,000 ticks, 2,000 bot actions, 0 refusals,
  1,126 save audits, result clean.
- **Replays with a captured frame.** Input logs written by hand (a walk
  command, then ticks) and `--replay-frame`, in the Vanilla 640x480 view
  and the widescreen view at 0.5 and 1.0 zoom.
- **Every exit, both ways.** The player was walked to each of the five
  converted doors in hops of 15 hexes, through the exit grid, and out of
  the interior stub again.
- **`mojave-convert probe walk`** (new): reads a map of the written layer
  and floods from its entry hex by the engine's movement rule.

What works:

- The layer mounts, the map loads (5,043 objects with the player) and
  draws: ground, wall strips, scenery, the engine's own interface.
- Wall strips sort correctly against the player, poles, fences and trees
  in every frame looked at, and the engine's see-through circle opens the
  strips when the player stands behind a building.
- All five exits lead to their stub map and back to the exterior.
- Every engine warning in the logs also appears on a stock map with no
  layer (stock `maps.txt` and `worldmap.txt` oddities, two script-variable
  warnings at load). The layer adds none.

What breaks or surprises:

1. **Ten of fifteen doors have no exit**, the saloon's front door among
   them: the porch hexes are blocked by bounding boxes. The saloon is
   entered by its back door only.
2. **Two doors, one interior, one way back.** Both schoolhouse doors lead
   to `good_003.map`, and leaving it always arrives at the first door.
3. **An exit grid on a through route.** Walking to the arrival hex of the
   door at the south edge crosses that door's exit grids and loads the
   interior. The arrival hex is on the map's border row.
4. **Islands.** 26,880 hexes are reachable from the entry; the builder
   counted 28,318 walkable. 1,438 walkable hexes are cut off.
5. **Long clicks do nothing.** Walks of up to 60 steps worked; walks of 70
   and 89 steps to reachable hexes did not start. The engine's route
   search is capped, as the original's is, and this map's scattered
   blockers use the cap up sooner than an open map would.
6. **No roofs.** Behind a building the player shows through the roof in
   the see-through circle, which reads as standing on it.
7. **Seen in the engine's own frames:** ground tiles show as rectangular
   patches where the tile library substituted a near match; the flag is
   untextured white; painted rocks never hide the player; a doorway is
   about 1.35 player heights tall where 1.15 is right, so the scale is
   about 15 % large, as estimated before.

To repeat it (the engine needs no audio library built this way):

```
git -C <vault-engine> archive HEAD | tar -x -C <scratch>/src
cargo build -p vault-platform --no-default-features   # in <scratch>/src
```

```
printf 'layers = ["mods/mojave"]\n' > out/goodsprings/load_order.toml
vault-platform --autoplay-headless --game-profile fallout2 --master-dat <f2>/master.dat --critter-dat <f2>/critter.dat --mod-list out/goodsprings/load_order.toml --preview-map maps/goodspri.map --ticks 2000 --autoplay-output out/engine-check/run1
mojave-convert probe walk out/goodsprings/mods/mojave goodspri.map --hop 15
```

To see it in a window, the same flags without `--autoplay-headless`
(a build with the default features, on a machine with a display).

### Door fix, same day

Finding 1 is fixed. The cause was two rules meeting: the slicer blocked
every hex inside a building's bounding rectangle, porch included, and the
builder puts a door's exit grids only on walkable hexes next to it.

- `map-slicer`: `SlicerConfig::open_hexes`. A sliced object's footprint
  does not block these hexes, and they never merge into a run, so each
  keeps a strip of its own that sorts against whoever stands there.
- `map-builder`: `walk::standable_hexes` (in a cell with a navmesh, under
  a triangle, within the slope limit) and `walk::verdicts` (the reason for
  every hex). A skipped door now reports its neighbours by reason.
- `mojave-convert`: the open hexes are the standable ones within 6 steps
  of a teleporting door. Everywhere, the walls list overflows (3,195
  against 2,398 free); near doors it costs 219 entries (2,048 to 2,267),
  leaving 131.

Result on the real map: 15 of 15 doors convert; 28,644
walkable hexes, 27,341 reachable from the entry; all 43 exit grids
reachable and no arrival point cut off (`probe walk`). In the engine each
of the 15 doors was walked in and out again, and autoplay ran 2,000 ticks
clean, ending inside the saloon stub.

Left as found: on a porch the see-through circle cuts the porch floor
away with the walls in front, so the player stands in a patch of bare
ground; and the player's feet are at ground level, not porch level.

### Returns, same day

Finding 2 is fixed. Stub maps were one per interior, and a stub's exit
ring can name only one arrival hex, so every door of an interior came
out at its first door. Four interiors had more than one door (ten doors
in all; the saloon has four). `map-builder` now makes one stub per door,
numbered in door order: 15 stubs, maps 152 to 166, six more small files.

In the engine each of the 15 doors was walked in and out again, and the
frame after leaving was compared with the view expected at that door's
own return hex: twelve match on 98 % or more of the sampled pixels, the
other three line up at
the expected position under a different light level, one of them with
the camera held back by the map's south edge. Autoplay ran 2,000 ticks
clean.

## 6. Owner playtest and the fixes from it (2026-11-20)

The owner played Goodsprings in a window. Their findings, and what was
done:

| Finding | Outcome |
|---|---|
| Roofs and the see-through circle are fine | nothing to do |
| Scale is right | scale tuning dropped from the backlog |
| On porches the circle showed dirt, not planks | fixed, below |
| The ground could be more detailed | sharper sampling shipped; the real limit recorded, below |
| Movement felt slow next to the enhanced preset | `scripts/play.sh` runs the enhanced preset from a release build |

**Porch floors.** The engine cuts wall and scenery art in front of the
player inside the circle unless the object carries a translucency bit;
`NO_TRANSLUCENCY` (0x8000) stops the cut with a normal draw, and `FLAT`
(0x8) draws an object under every character. Porch planks were ordinary
wall-strip pixels, so they were cut.

- `map-slicer` (`floor.rs`, `Role::FloorPiece`): for a sliced object with
  open hexes in its footprint, the triangles facing up and 8 to 80 units
  above the terrain are rendered on their own (which also finds the
  planks hidden under a porch roof), cut into pieces over hexes within 2
  steps of an open hex, and cleared from the strips.
- `emit`/`map-builder`: a floor piece is scenery art placed `FLAT |
  NONBLOCKING | NO_TRANSLUCENCY` (`ObjectFlags::FLOOR`).
- Real map: 177 floor pieces, scenery list 426 to 603 entries. Engine
  frames at the saloon and general store porches show planks under the
  player. All 15 doors walked in and out, autoplay 2,000 ticks clean.

**Ground detail.** Two things were tried.

- Kept: land textures are filtered one mip level finer
  (`TerrainOptions::texture_lod_bias`, -1 in the map stage; a
  `texture_sharpen` option also exists, unused because it reads as
  noise). Cracks and grain are visibly crisper in engine frames.
- Tried and removed: making tiles repeat by shading each square in steps
  and by rounding layer blends. Of 10,000 squares, 9,992 are unique
  today; with shading in 8 steps 9,902 were, and with blends rounded to
  all-or-nothing as well, 9,676. The combinations (16 texture offsets,
  several textures, blends, baked overlays) are simply more than 977
  tiles, so substitution stays, and with it the rectangular patches.
- What would help is more tile entries. The layer already ships the whole
  tile list, so it could overwrite Fallout 2's own entries (about 4,094
  instead of 977), at the price of stock maps looking wrong while the
  layer is mounted. That is an owner decision, and the one tile list is
  shared by every future map, so it should be made before more maps are
  converted.

**Launcher.** `scripts/play.sh <engine> <fallout2-dir> <out-dir> [map]`
mounts the layer with `--preset enhanced` and a save folder of its own.
The owner's Fallout 2 profile has no preset set, which is why the first
session ran vanilla.

## 7. Townsfolk: the first M4 wave (2026-11-20)

Goodsprings now has people. The contract is `docs/contracts.md` section
10; this is what was built and what it shows.

| Wave | Agents (Opus) | Crates |
|---|---|---|
| M4a | 3 | `nif` (skeleton, skin, animation, posing, CPU skinning); `fnv-esm` + `mir` (actor documents); `emit` (critter files) |
| M4b | 2 | `mir` (skeleton and animation stores); `map-builder` (critter placement) |
| lead | | `mojave-convert`: the `critters` stage, character contact sheets, `critters --preview` |

**Pipeline.** `all` is now extract, sprites, critters, map, gallery. The
critters stage poses each distinct human appearance from the game's
skeleton and animations, renders six facings, and writes a sprite set:
a real stand (20 frames, from the 4 s idle) and a real walk (12 frames,
one 1.2 s cycle), plus nine one-frame placeholders so a fight cannot
break the engine. Sets are cached by content under `critters/`.

**Real slice.** 42 actor documents (23 people, 19 creatures), all
resolved; 23 sprite sets rendered, none failed. Three townsfolk stand in
the map window: Easy Pete on the saloon porch and two settlers. The
other seven placed there are quest-only (an enable parent keeps them
away) or start disabled. The named residents are inside interiors.

**In the engine** (headless): Easy Pete draws on the porch beside the
player, hat on; autoplay ran 2,000 ticks clean; no new warnings.

**Found and fixed while looking at enlarged previews:**
- Hats and hair lay at the feet. They are rigid pieces modelled upright
  around the head bone's origin; they now follow the head, with the
  bone's rest rotation undone.
- Outfits carried the game's hidden gore caps (dark red at neck, arms
  and knees). Shapes named `MeatCap*` or textured from `textures\gore\`
  are dropped.
- Hair was untinted and drawn twice. The actor's hair colour is
  multiplied in, and the under-a-hat copy of the hair is dropped.

**Limits, all visible in play or in `gallery/critters-NN.png`:**
- They only stand and fidget. The engine walks a critter only in combat
  or by script, so walking waits for M5.
- A figure is about 62 pixels tall: true to the map's scale, so shorter
  than the Fallout 2 hero beside it. The owner found the first render "a
  tad dark and thin"; figures are now drawn 15 % wider about their
  upright axis and with a gamma of 0.78 before quantizing (`GIRTH` and
  `LIFT_GAMMA` in `mojave-convert`'s `critters.rs`). Height is unchanged.
- Exposed skin on an outfit's own arms uses the default skin, not the
  race's. No FaceGen, beards or eyes.
- Hit, fall, get-up and corpse art are the standing figure.
- Creatures have documents and skeletons but no sprites.
- AI packet 179 was picked from the base table by rule and team 99 by
  default; contract 10.5 still has the three owner questions.

## 8. Where work stopped (2026-11-20)

The owner played the build with townsfolk and called it a good stopping
point. State at the stop:

- Gates green: 534 tests, clippy and format clean. Nothing is committed.
- The real output is under `out/goodsprings/` (git-ignored); play it with
  `scripts/play.sh <engine> <fallout2-dir> out/goodsprings`. The engine
  copy used for checks is a release build of `vault-engine` at `df2bf4c`
  under `target/engine-head/` (no audio: the container lacks ALSA headers).
- `vault-engine` was never edited. `vault-formats` is still pinned at
  `3adeac9`; nothing built so far needed a newer one.

Seen in the owner's last screenshot, not yet acted on:

- **Look-at text shows source paths.** "You see: Floor (meshes\architecture\...)"
  and "Wall (meshes\vehicles\motorcyc...)": prototype names are built
  from the model path. They should be plain names, and floor pieces and
  wall strips probably should not answer a look at all.
- The parked motorcycles are sliced into wall strips; they would be
  better as single scenery sprites.

Decisions waiting on the owner:

1. Commit the repository (it has never been committed).
2. Tile budget: let the layer overwrite Fallout 2's own floor tiles for
   about four times the ground detail, at the price of stock maps looking
   wrong while it is mounted (section 6).
3. The three character questions in `docs/contracts.md` 10.5: AI packet
   and teams, creatures in the next wave, whether townsfolk may be killed
   before death art exists.

Next work, in the order it pays off:

1. Plain look-at names; motorcycles and similar props as sprites.
2. Real interiors, so the named residents (Doc Mitchell, Trudy, Sunny)
   have somewhere to stand: today each door leads to a placeholder floor.
3. M5 scripts, which is what makes anyone walk or talk. It waits on the
   INT assembler brief landing in `vault-engine`.
4. Creatures (other skeletons), real hit and death art, faces.
5. The older backlog in section 4: trees, untextured surfaces, occluding
   rocks, cut-off islands, more map windows, a world-map entry.

