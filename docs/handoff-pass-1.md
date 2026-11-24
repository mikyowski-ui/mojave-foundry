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

- Gates green: 534 tests, clippy and format clean. This state was first committed on 2026-11-20 (`9d9d50d`).
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

## 9. Demonstration maps, second pass (2026-11-22)

Four proof-of-concept areas were converted again with `--standalone`
after the landmark, default-texture and islands work in `map-slicer`,
`mir`/`map-terrain` and `map-builder` was wired into `mojave-convert`.
Output is under `out/poc/{helios,strip,primm,nellis}/` (git-ignored),
the chosen frames under `out/poc/best/`. Everything ran headless, one area
after another, against the release engine under `target/engine-head/`.

**Wired in (`mojave-convert`).** `map-report.md` gained `## Open hexes`,
`## Entrance`, `## Components`, `## Passages` and `## Islands left`, a
`## Ground` summary with the default-texture counts, every landmark tally,
and each exit's grid steps. `SlicerConfig::landmark_walkers` is every hex
`walk::verdicts` calls walkable with nothing blocked;
`BuildOptions::protected_hexes` is every hex of the slicer's merged runs
(the slicer does not report a landmark's blocked core apart from relief,
so that is left to the builder). A standalone run opens every standable
hex and also takes statics whose origin is outside the window but whose
footprint reaches in; a run without the flag is unchanged. The map
stage's key is `map-7`. `map-features.json` leads with the four largest
landmarks and never drops one whose centre is off the grid; `shots --auto`
takes a model twice only after every other. `mojave-convert contact` lays
PNGs out on a labelled page.

**Windows.** Nellis moved to `(32000, 145800)`, cells x 5..10, y 32..38.
The Strip moved from `(0, 3000)` to `(-1200, 3100)`, cells x -4..2: the
Tops' west hotel and casino wings, whose origins lay outside the old
window, are now on the map; the old east strip (x 5,800 to 7,000: 16
minor references, palms, planters, wall pieces) is lost. A window's north
edge gives a tall building `32 * t` pixels of headroom (t: its distance
from that edge in floor squares), whatever its x, so Gomorrah's and the
Lucky 38's tops cannot fit in any window that also holds Vault 21 (12,300
units further south). A trial 500 units further north
(`out/poc/strip-north/`) kept the same eight relief fallbacks and lost
every exit, so the chosen centre stands. HELIOS One and Primm are as
before.

| | HELIOS One | Strip | Primm | Nellis |
|---|---|---|---|---|
| references in the window | 735 | 1,323 | 1,379 | 571 |
| walls added (of 4,096) | 3,154 | 3,929 | 3,714 | 3,115 |
| scenery added | 619 | 1,780 | 1,909 | 1,049 |
| floor tiles | 4,089 | 4,079 | 4,081 | 4,078 |
| budget ladder steps | 2 | 3 | 3 | 2 |
| landmarks: references, pieces | 1, 156 | 39, 956 | 61, 1,459 | 1, 158 |
| landmarks baked as relief instead | 0 | 8 | 4 | 1 |
| doors converted | 4 of 5 | 16 of 17 | 10 of 10 | 14 of 14 |
| reachable from the entrance | 28,966 of 28,998 (99.9 %) | 5,221 of 17,925 (29.1 %) | 24,521 of 28,142 (87.1 %) | 23,372 of 23,501 (99.5 %) |
| exits unreachable | 0 | 3 | 0 | 0 |
| autoplay, 2,000 ticks | clean, 1,939 audits | **failed** at tick 967 | clean | clean, 1,938 audits |
| `all` (s) | 73 | 246 (sprites cached) | 188 | 163 |

First pass, for comparison (same order): reachable unknown, 2,743 of
15,120 (18 %), 15,499 of 23,392 (66 %), 17,918 of 20,591 (87 %, the same
Nellis window); doors 3 of 5, 11 of 17, 9 of 10, 14 of 14; every Strip
exit was unreachable.

**Open hexes everywhere** (standalone), measured against the same build
with only the hexes near doors open (`out/poc/neardoors/`): reachable
share 55.8 to 99.9 % (HELIOS), 73.5 to 87.1 % (Primm), 86.1 to 99.5 %
(Nellis); on the Strip 0 to 13 of 16 exits reachable, 5,117 to 5,221
hexes. Walls added: HELIOS 3,465 to 3,154 and Nellis 3,068 to 3,115 (the
open run climbs two ladder steps, the other none), Strip 3,872 to 3,929,
Primm 3,807 to 3,714. Landmark pieces grow (Strip 324 to 956, Primm 127
to 1,459), since more walkers must be sorted and the ladder promotes more
models.

**What works.**
- Landmarks draw in the 640 x 480 game view: the HELIOS power plant, the
  Lucky 38's base and the Nellis hangar, all missing from the first pass's
  game frames.
- The Strip's ground is textured (100 land layers drew with the default
  land texture; the first pass showed flat colour).
- The Nellis black patch is gone: it lay under the far end of the control
  tower, whose origin is 2,200 units south of the window, so the map
  stage never handed it to the slicer (the slicer was not the cause; the
  runway connectors are classified as paved slabs and drawn flat). The
  tower is now on the map as wall strips.
- HELIOS and Nellis are almost wholly reachable, autoplay is clean there,
  and all exits but three on the Strip are reachable.

**What does not, most visible first.**
1. *Tall landmarks baked flat.* A landmark with any piece beyond the
   engine's reach above the map is baked whole as relief
   (`no hex near piece`): Gomorrah, the Lucky 38's top and lights, the
   Ultra-Luxe front and the monorail platform (Strip), the El Diablo
   coaster (Primm), the north hangar (Nellis). They lie on the ground as
   noise and the player walks over them. `map-slicer` (`landmark.rs`).
2. *The player stands on roofs.* Opening every standable hex opens the
   navmesh inside open-fronted buildings (hangars, arcades) and under
   roofs; the player is drawn over a flat roof (Strip entry), or walks
   under a landmark roof inside the see-through circle (Nellis hangar,
   HELIOS plant), and crates placed inside a hangar draw over its roof.
   `mojave-convert` policy and `map-slicer` (pieces do not sort against
   sprites).
3. *The Strip splits into islands*: 43 components, the entrance's holds
   29 %; the largest island (2,637 hexes, two exits) needs an 8-hex
   passage where the limit is 6; the Lucky 38 door has no walkable
   neighbour. `map-builder`.
4. *Autoplay fails on the Strip*: `AuditMismatch` at tick 967 entering
   `STRI_011.SAV` ("the locals differ"), after 67 clean transitions;
   repeatable. Engine or `map-builder` stub maps, undiagnosed.
5. The game view's camera stops short near the map's edges, so several
   game frames repeat one view (HELIOS 3 to 7). Known since the first
   pass.

**Deliverables.** `out/poc/best/<area>-1..4.png` (three wide frames and one
640 x 480 frame per area) and `out/poc/best/contact.png`. The real
Goodsprings layer, rebuilt without `--standalone`, differs from
`out/baseline-goodsprings.sha` in six tile images and the floor words of
15 squares of `maps/goodspri.map` only (46 squares drawn differently);
`out/baseline-goodsprings-2.sha` holds its new checksums.

## 10. Interiors (2026-11-22)

`mojave-convert` builds a New Vegas interior cell as a standalone,
walkable demo map with its people standing in it, on the enablers added
the same day (`GroundSampler::flat` and `mir::storeys`;
`TerrainOptions::blank_without_land`; `SlicerConfig::interior`;
`BuildOptions::interior`). Each room is its own layer with its own
`--out`, built `--standalone`: one storey, doors drawn but leading
nowhere, no interior lighting, no creatures. The pilot is the Prospector
Saloon; the slices for Vikki & Vance, the Ultra-Luxe and the Tops are
written and parse, but are not built yet. The interfaces are in
`docs/contracts.md` 9.5, the slice keys in `docs/running.md`.

**Converter changes.** New slice keys `interior`, `floor_z`,
`map_center` (room coordinates), `entrance`, `place_disabled`,
`clamp_levels`, `cut_height`, `floor_band`, `hang_clearance`,
`walk_band`, validated (interior-only keys on an exterior slice are an
error). An interior slice without `--standalone` stops before extract.
`map-report.md` opens with `## Interior`; `map-features.json` gains the
room and a ranked `people` list; `shots --auto` aims at the entrance, the
people, then the walls, with interior defaults for `--spread` (5; people
1) and the new `--wide-zoom` (0.75). Exterior output is unchanged: the
Goodsprings layer rebuilt with default options matches
`out/baseline-goodsprings-2.sha` (6,866 of 6,866 files). Map stage key
`map-8`.

**Saloon numbers** (`out/interiors/saloon/`, `slices/saloon.toml`, cut
150):

| | |
|---|---|
| references in the room | 461; 120 left out by the slicer (45 above the cut, 24 with no mesh, 23 effects and markers, 13 empty renders, 7 statics starting disabled, the rest minor) |
| floor | z 3456.9, the one navmesh storey (z 3450 to 3473) |
| walls added (of 4,096) | 263 |
| scenery, items added | 110, 89 |
| floor tiles | 116 distinct (117 in the library with the blank tile) |
| floor baked | 2,892 triangles from 92 references, 324 hexes |
| people placed | 8 of 8: Sunny Smiles, Trudy, Joe Cobb, Goodsprings Settler, four Powder Gangers (six of them start disabled in the data) |
| left out | Cheyenne (a creature: not rendered) |
| walkable, reachable from the entrance | 207 walkable; 199 reachable (the rest hold the 8 people), one component |
| doors | 4 teleport doors drawn, none converted |
| autoplay, 2,000 ticks | clean, 557 actions, 0 refusals |
| `all` | 40 s from an empty `--out` (extract included), 785 MB peak |
| `shots`, 14 viewpoints | 8 s, every viewpoint arrived |

**Ceiling cut.** Three builds of the same room compared on engine frames
(`out/interiors/best/saloon-cuts.png`; builds under
`out/interiors/saloon-cut210/` and `saloon-cut120/`): at the default 210
the partitions between bar and pool room stand full height and hide the
pool table and two people; at 120 everything opens up (the restrooms'
fittings show) but the jukebox is cut through; 150 keeps the jukebox and
opens the room. The saloon slice uses 150. The other rooms keep the
default for now: compare 150 there too.

**Best frames.** `out/interiors/best/saloon-1.png` (wide view, the whole
room and all eight people, the player by the pool table),
`saloon-2.png` (wide, from beside the Settler at the bar), `saloon-3.png`
(640 x 480 game view, the whole room, eight people), `saloon-4.png`
(game view by Sunny Smiles and two Powder Gangers, pool table and
jukebox), and `saloon-contact.png`.

**Recipe for another room** (the frozen binary is
`out/bin/mojave-convert-int`; one conversion at a time):

```
B=out/bin/mojave-convert-int
E=target/engine-head/target/release/vault-platform
F2=<your Fallout 2 folder>
FNV="<your Fallout New Vegas folder>"
O=out/interiors/<room>
$B all --standalone --slice slices/<room>.toml --jobs 3 --fnv "$FNV" --f2 $F2 --out $O
$B estimate --standalone --slice slices/<room>.toml --fnv "$FNV" --f2 $F2 --out $O   # optional
printf 'layers = ["mods/mojave"]\n' > $O/load_order.toml
$E --autoplay-headless --game-profile fallout2 --master-dat $F2/master.dat --critter-dat $F2/critter.dat \
   --mod-list $O/load_order.toml --preview-map maps/<map>.map --ticks 2000 --autoplay-output $O/engine-check/autoplay
$B probe walk $O/mods/mojave <map>.map --hop 15
$B shots --engine $E --f2 $F2 --out $O --auto 8
$B shots --engine $E --f2 $F2 --out $O --at <hex> --at <hex> ...   # one per person
```

`<map>` is the slice name cut to eight characters (`saloon`, `vikki-va`,
`ultra-lu`, `tops`); the people's hexes are in `map-report.md`
(`## Interior`, "People placed") and `map-features.json`. `shots` writes
over `<out>/shots/` each run, so copy it away before a second run. Read
the report's `## Interior` first: the floor, the storeys found, who was
left out and why. For the Tops, check the clamped people (the builder
lines "from another level") and whether the sunken area reads.

**What looks wrong, by owner.**
1. `map-slicer`: one cut for everything. A lower cut that clears
   partitions also cuts tall furniture (the jukebox at 120); room shells
   and free-standing objects would want separate cuts
   (`out/interiors/best/saloon-cuts.png`).
2. `map-slicer`, data-faithful: the back office (south) is crossed by log
   beams (`camplog03`, 95 to 110 units up) and the studs of the shack
   walls, which read as clutter at every cut
   (`out/interiors/saloon/shots/saloon-12-game.png`).
3. Unclear: inside the see-through circle in the back hall by the stairs
   the floor is very dark; the floor bake or the hall's own dark floor
   (`out/interiors/saloon/shots/saloon-09-wide.png`).
4. `mojave-convert`: a generic name used once reads "You see Goodsprings
   Settler." (only names shared by several people take an article).
5. Critter stage / `map-builder`: creatures are not rendered (Cheyenne).
6. Not seen as a problem here: ceiling clutter over the floor (none at
   150), floor seams, people inside furniture, the camera stopping short
   (the room sits in the middle of the grid), walls hiding the player (the
   see-through circle works in the back hall). People are lit like
   outdoors and read a little dark against the dark room.

**Not verified.** Look-at text in a window (only the message files were
read: `pro_crit.msg` names all five appearances); the three other rooms;
a window build of the engine (all runs headless).

## 11. Fallout 3 first, both games in one pipeline (2026-11-23)

The owner made Fallout 3 the first target. `mojave-convert` now converts
a Fallout 3 or a New Vegas install through one front end, on the
parser work of the same day (`fnv_esm::GameVariant`,
`mir::extract_for_game`, the logic tables, `Records::referenced_refs`,
child worldspaces on their parent's land). New Vegas output is unchanged.
Commands and keys are in `docs/running.md` ("Which game", "Fallout 3
slices", "Declutter and tone"); interfaces in `docs/contracts.md`
sections 1 and 9.

**Front end.** `--game-dir <install>` with the game read from the master
in `Data` and confirmed by its header; `--game fo3|fnv`; `--fo3` and
`--fnv` as aliases that fix the game (`--fnv` behaves exactly as before
on a New Vegas install). Slices may say `game = "fo3"|"fnv"` (default:
the master their plugins start with); built-in slices are per game
(`goodsprings`, `springvale`), and a run without `--slice` takes its
game's. Every disagreement (flag, `--game`, slice, install, the first
plugin's header) stops the run before extract and names both games
(`ConvertError::Source`, which replaces `ConvertError::Fnv` and carries the
flag that named the install). The game is in `mir/report.md` and
`manifest.json` (parser crate), `map-report.md` (second line) and
`map-features.json` (`game`); from Fallout 3 on it is hashed into the
sprite and map stages' keys (New Vegas keys unchanged). The critter
stage's key is not mine to change (`critters.rs`); its sprite sets are
keyed by content already. `probe esm` prints the plugin's game, every
decoder of both tables, and `--cells <text>` breaks a cell's placed
references down by base record type. The slicer starts from a per-game
pattern profile (`map::slicer_defaults`; New Vegas = the slicer's
defaults).

**Slices** (`slices/fo3/`, editor ids and cells checked against
`Fallout3.esm` with the probe and an extract): `springvale` (Wasteland,
x -5..0, y -6..-1, centre (-7100, -11200) among the ranch houses; the
brief's x -4..-1, y -5..-1 cannot hold a 14,000-unit window plus one
cell), `super-duper-mart-exterior` (x 1..6, y -7..-2, centre (17400,
-16800) in cell (4, -5); the brief's three by three cells leave the
window's west and south strips without land), `super-duper-mart`
(interior `SuperDuperMart`, cells 3..5 x -6..-4, both doors in (4, -5)),
`megaton` (`MegatonWorld`, x -2..0, y -8..-6: the worldspace has no other
cells; the window reaches past them), `megaton-saloon`
(`MegatonMoriartysSaloon`, cut 120).

**Runs** (each `all --standalone --jobs 2` from an empty `--out` under
`out/fo3/<slice>/`, then a 2,000-tick headless autoplay, `probe walk` and
`shots --auto 8 --classic --wide`; binary `out/bin/mojave-convert-dual`):

| | Super-Duper Mart | Moriarty's Saloon | Megaton | Springvale | Super-Duper Mart, outside |
|---|---|---|---|---|---|
| references in the window | 1,767 | 458 | 937 | 1,632 | 1,003 |
| walls / scenery / items added | 2,331 / 230 / 407 | 177 / 50 / 59 | 3,861 / 568 / 25 | 3,490 / 888 / 69 | 3,127 / 1,050 / 25 |
| floor tiles (of 4,094) | 971 | 67 | 4,093 | 4,077 | 4,087 |
| budget ladder steps | 0 | 0 | 3 (11 landmarks, 330 pieces; 3 border walls baked as relief) | 0 | 0 |
| people placed / left out | 8 / 3 (a Protectron; two raiders on the upper level, z +174) | 6 / 0 (Moriarty, Gob, Nova, three settlers) | 3 | 0 | 2 |
| walkable / reachable | 2,760 / 2,751 | 116 / 110 | 8,922 / 3,350 (37.6 %, 20 components) | 26,421 / 26,292 (99.5 %) | 33,164 / 33,151 |
| doors | 21 door references; 2 teleport doors drawn, not converted (demo) | 7; 2 teleport doors drawn | 18 teleport doors: 15 exits, all unreachable; 3 not converted | 3 exits, all reachable | 2 exits, both reachable |
| autoplay, 2,000 ticks | clean, 1,950 actions | clean, 129 actions | clean, 23 actions | clean, 2,000 actions, 1,925 audits | clean, 26 actions (left by an exit) |
| `all` | 32 s, 707 MB | 89 s (5.8 s rebuilding at cut 120) | 184 s | 126 s, 815 MB | 99 s, 757 MB |
| shots arrived | 16 of 16 | 16 of 16 | 2 of 2 (one viewpoint) | 16 of 16 | 16 of 16 |

The extract's own counts match the brief: the Super-Duper Mart room holds
10 NPC, 1 creature, 68 container, 2 terminal and 21 door references;
Moriarty's 6 NPC, 20 container, 1 terminal. Extracts had no undecodable
records.

**Best frames** (`out/fo3/best/`, with `contact.png`):
`super-duper-mart-1.png` and `-2.png` (wide: shelves, freezers, the
raiders), `-3.png` (classic, the back offices), `megaton-saloon-1.png`
(the bar, its stools and all six people at cut 120) beside
`megaton-saloon-cut210.png` (the default cut: joists and the balcony
over everything), `springvale-1.png` (ruined houses, the water tower and
the Red Rocket), `springvale-preview.png`, `megaton-preview.png` (the
map stage's composite: the town reads, the engine cannot reach it),
`super-duper-mart-exterior-1.png` (the store's side and its car park) and
`-preview.png`.

**What looks wrong, by owner.**
1. `map-builder`: Megaton is a town of walkways and ramps at several
   heights in a crater. Flattened to one level, the inside splits into
   islands (3,259, 1,275, 405, 340 hexes ...), and the entrance lands in
   the largest component, which is the ground outside the wall; every
   exit is unreachable, autoplay does nothing and `shots` gets one
   viewpoint. An exterior slice needs a way to name its entrance
   (`entrance_near` exists for interiors only) and multi-level walkways
   need their own handling.
2. `map-slicer`: the player stands on top of the Red Rocket's canopy in
   Springvale (`springvale-2.png`): navmesh under a landmark roof, the
   known "player on roofs" item of section 9.
3. `map-slicer`/`foundry-render`: shopping carts (wire meshes) render as
   brown scribbles on the Super-Duper Mart floor; the store's roof sign
   letters render as empty frames in the exterior preview.
4. `map-slicer`: Megaton's border walls (`megatonborderwallreg02`) are
   baked as relief (`no hex near piece`), the tall-landmark item of
   section 9.
5. Fallout 3 trees are SpeedTree `.spt` models, which have no mesh: they
   are skipped as "no mesh" (62 in the Super-Duper Mart cells). The dead
   trees that do show are static meshes.
6. Critter stage: creatures (the Protectron) are not rendered, as before.
7. Data, not a defect: two Super-Duper Mart raiders stand on the upper
   level; `clamp_levels = true` would place them.

**Regressions.** Goodsprings with defaults (`all --fnv ... --out
out/goodsprings`, 344 s) and the Prospector Saloon (`all --standalone
--slice slices/saloon.toml`) match their baselines in every file outside
`art/critters/`: maps, prototypes, walls, tiles, scenery, lists and
messages are byte-identical. The critter art differs and grows (new
animation sets, `mj0000ak.frm` and others) because of the characters
agent's work in `critters.rs`, which `README.txt`'s file list follows.

**Determinism.** The Super-Duper Mart built twice into fresh `--out`
directories (`out/fo3/det-a`, `det-b`): all 6,988 files under
`mods/mojave` identical, and `map-features.json` too. `map-report.md`
differed only in the terrain stage's timings inside `## Ground`; the
report now leaves those lines out.

### Declutter, tone and interactive wiring (same day)

On the declutter agent's `SlicerConfig::declutter` and the builder's
`BuildOptions::interactive` and `BuildReport::readability`. Off by default
for both games (`mojave_convert::declutter::default_profile`), so every
slice keeps its bytes; `--declutter readable` (on `map`, `all`,
`estimate`) or `[declutter] profile = "readable"` turns it on from the
game's preset (`readable_for`: New Vegas the slicer's `readable()`,
Fallout 3 adds paper and litter heaps and shopping carts to the clutter
patterns). The `[declutter]` keys (`protect_items`, `protected_kinds`,
`protect_referenced`, clutter thresholds and extra patterns, `density`,
`decals`/`decal_fade`, `overhang`/`overhang_clearance`,
`shell_cut`/`shell_cut_height`, `cutaway`/`kerb_height`/
`cutaway_exteriors`, `outline`/`outline_factor`, `measure_baseline`) and
`[tone.floor|props|figures]` (`gain`, `gamma`, `saturation`, `contrast`)
are mapped onto the profile by hand (its types are not serde types); keys
with the pass off are refused before extract. `protected_refs` is
`Records::referenced_refs()` (14,274 references in `Fallout3.esm`,
12,255 in `FalloutNV.esm`); the builder's interactive set is those plus
every window reference of a protected or item kind and every teleport
door. The profile name and the preset are in the map stage's key when
the pass is on. `map-report.md` has `## Declutter` and `## Readability`
(the latter on every run).

| | Prospector Saloon | Super-Duper Mart | Moriarty's Saloon |
|---|---|---|---|
| pruned (clutter, pattern, density, overhang) | 28 (17, 3, 1, 7) | 214 (10, 202 of which 89 carts, 0, 2) | 22 (10, 11, 0, 1) |
| protected | 155 | 814 | 123 |
| cutaway: strips cut, walkers hidden before / after | 117 of 186, 135 / 7 | 448 of 939, 635 / 0 | 53 of 91, 47 / 0 |
| open floor before / after | 26.3 % / 43.4 % | 41.2 % / 56.8 % | 33.3 % / 36.4 % |
| walkable hexes hidden before / after | 56.5 % / 29.7 % | 46.2 % / 27.9 % | 42.2 % / 37.1 % |
| reachable, current / readable | 199 / 201 | 2,751 / 2,847 | 110 / 110 |
| autoplay, readable | clean, 578 actions | clean, 578 actions | clean, 141 actions |

With `protect_items = false` (`$TMPDIR` slice copies; the key is the
owner's choice): the saloon prunes 125 (112 by size: bottles and glasses
on the bar and tables), open floor 44.6 %, objects on 24.9 % of the
floor (29.3 % with items protected); the Super-Duper Mart prunes 480 (the
cans on its shelves), built before the cart pattern was added.

Frames (same viewpoint hexes before and after; the readable run's
entrance moves, so its viewpoint numbers are one higher):
`out/interiors/best/saloon-readable-pairs.png` and
`saloon-items-pairs.png`; `out/fo3/best/super-duper-mart-readable-pairs.png`,
`super-duper-mart-items-pairs.png` and `megaton-saloon-readable-pairs.png`;
runs under `out/interiors/saloon-readable{,-items}/` and
`out/fo3/{super-duper-mart,megaton-saloon}-readable/`,
`out/fo3/super-duper-mart-readable-items/`.

**What works.** The saloon's partitions and front walls drop to the
kerb: the booths, a person behind the south wall and the restrooms show,
the back office's beams are gone. The Super-Duper Mart loses its planks,
litter and carts and reads as a store floor; every raider stands in
view. Moriarty's front railing opens.

**What looks wrong, by owner.**
1. `map-slicer`: doors stand full height inside walls cut to the kerb
   (the saloon's south and east doors in every readable frame); the
   grocery shelves are long and thin enough to count as walls, so the
   cutaway cuts the aisles to low outlines (`super-duper-mart-readable-pairs.png`,
   top right). A cutaway exclusion pattern (or a `max_height` for walls)
   would let a slice keep furniture whole; I can map it to a slice key.
2. `map-slicer`: an item the slice leaves unprotected loses its outline
   too (outlined sprites 111 to 17 in the saloon); outline and protection
   look tied.
3. `map-builder`: `ReadabilityReport` calls 72 to 100 % of walkable hexes
   covered (0 clear in both saloons) where the slicer's floor metric
   finds 28 to 37 % hidden after the pass; the builder's cover test seems
   to count something that is not upright art over the figure.
4. Pool table and bar: they are sliced (wall strips), not sprites, so
   they do block in principle, but a strip does not block a hex in
   `SlicerConfig::open_hexes`, and the interior run opens every hex the
   navmesh rasterises (`walk::standable_hexes`, tolerance 24 units, hexes
   about 70 across). On the written saloon map 8 of the 12 hexes under the
   pool table and 3 of 6 tested under the two bar pieces are free and reachable
   (`probe walk --from`), though no navmesh triangle reaches the table's
   centre. Owner: `map-builder` (which hexes count as standable) with
   `map-slicer` (open hexes override furniture); from here I could
   narrow the open set indoors, which would change every interior's bytes.
5. Not done from here: the floor tone does not reach exterior terrain
   tiles (`map-terrain` quantizes inside `build_ground`, with no hook
   before it), and the figures tone is not applied to critter sprites
   (`critters.rs`, the characters agent's).

### Phase B, first half: items, loot, armed and sided people (same day)

On the builder's `map_builder::loot` (`Catalogue`, `verify_loot`), its
furniture footprints and `emit::rebalance`. Interfaces in
`docs/contracts.md` 9.6 and 10.6, commands and keys in `docs/running.md`
("Loot, sides and the rebalance tables", "Critter stage options", "Engine
checks of a fight"). New code: `mojave_convert::rebalance` (the tables),
`mojave_convert::catalogue` (records to a `Catalogue`), the map stage's
wiring, `CritterOptions::for_run`, `gallery::items_page` and
`before_after_page`, `probe f2` listing the base `ai.txt` packets, and
the subcommands `replay-frames` and `before-after`.

**Switches.** Slice keys `loot`, `animations`, `deaths`, `stride`,
`figure_look`, `footprints`; flags `--loot on|off` (map, all, estimate),
`--rebalance <file>` (every stage), `--animations`, `--deaths`,
`--stride`, `--figure-look`, `--only` (critters, all). Loot is off and the
level minimal unless a slice says otherwise; the five Fallout 3 slices say
`loot = true` and `animations = "combat"` (the rooms also readable
declutter; the Super-Duper Mart footprints). Above minimal an interior
slice renders only the people its room places (Moriarty's rendered all 43
Megaton sets, 431 s, before this; the saloon now renders 5 in 87 s).
`[tone.figures]` and the readable look need a level above minimal.

**Weapon classes** (`[convert.weapons]`, both games): animation type 0
unarmed, 1 blunt, 2 heavy melee, 3 pistol, 4 energy pistol, 5 rifle, 6
assault rifle, 7 energy rifle, 8 minigun, 9 launcher, 10 to 12 grenade,
13 thrown; with the energy weapons skill (34) a pistol, rifle or minigun
takes its energy class; rules then make knives, swords, axes blades,
spears spears, shotguns and SMGs theirs, flamers flamers (fire);
plasma, Tesla, pulse by name; launchers, grenades and mines explosive.
The `DNAM` automatic flag gives a single-shot class a burst.

**Sides** (`[convert.hostility]`): the AI data (`AIDT` byte 0) and
factions (`SNAM`) of the actor that supplies them through its templates
(flags 0x10 and 0x04, leveled NPC lists by their first lowest-level
entry), against the player faction's relations (`XNAM`, either way).
Aggression 2 or 3 is hostile unless a faction is the player's friend or
ally; aggression 1 is hostile only with a faction that is the player's
enemy. Fallback patterns (raider, talon, super mutant, slaver, feral;
fiend, viper, jackal for New Vegas) are used only without AI data; no
real actor needed them. Hostile people: team 66, packet "Merc Raider"
(136); friendly: team 99 and the builder's packet (179 in a stock
install).

**Runs** (binary `out/bin/mojave-convert-b2`; `all --standalone --jobs 2`
from an empty `--out`; then a 2,000-tick autoplay, a 4,000-tick
`--chaos-doors-c4` autoplay, `probe walk`, `shots --auto 8 --classic
--wide`, one `walkthrough`):

| | Super-Duper Mart (`out/fo3b/super-duper-mart`) | Moriarty's Saloon (`out/fo3b/megaton-saloon`) | Prospector Saloon, loot and combat (`out/fnvb/saloon`) |
|---|---|---|---|
| item prototypes | 77: 8 weapons, 3 armour, 7 ammunition, 17 aid, 1 key, 41 misc | 33: 2, 5, 2, 3, 1, 20 | 63: 8, 7, 5, 14, 0, 29 |
| icons; own ground sprites | 74; 26 (21 models read from the install, 1 placeholder: the bottle cap renders empty) | 32; 18 | 63; 28 |
| on the ground | 353 references | 45 | 95 |
| containers | 68, 30 filled, 48 stacks, 3 locked; 301 leveled lists drawn, 89 empty | 15, 5 filled, 8 stacks | 5, 4 filled, 10 stacks |
| doors | 21 (1 locked and keyed) | 3 (1 locked) | 6 (1 locked) |
| named placements | 128 | 38 | 41 |
| people | 10 in the data, all hostile raiders (aggression 2); 8 placed, all armed (6 `.32 Pistol`, 2 `Pool Cue`), all in Raider Painspike Armor | 6, all friendly (aggression 0 or 1); 4 armed | 8, all friendly (aggression 1; the Powder Gangers' faction is not the player's enemy); 7 armed |
| critter art | 2 sets, 44 files, 8.1 MB (23 and 21 files, 3.4 and 4.6 MB); rows 103 and 104 of `critters.lst`, 3,991 free | 6 sets | 5 sets |
| walkable / reachable | 2,125 / 2,108 of 2,117 | 116 / 110 (footprints off; 63 of 95 with them) | 207 / 199 |
| `all` | 53 s, 708 MB | 431 s (all of Megaton rendered; see above) | 87 s, 684 MB |
| autoplay 2,000 ticks | the bot fought the raiders and the player died at tick 1,455, 0 errors | clean, 397 actions, 1 fight | clean, 771 actions, 1 fight |
| chaos 4,000 ticks | clean: 6 fights ended, every raider attacked, 118 checks of doors and containers held, then "nothing left to do" | 25 checks held, then the stuck-loop guard (a room with no exit) | not run |

**Engine findings.** What runs headless: combat starts, but only because
the autoplay bot attacks every unscripted critter on another team
(`vault-core` `autoplay.rs`, `next_goal`); a Fallout 2 critter on a
hostile team and an aggressive packet does not attack the player by
itself (contracts 10.1), so in a person's game the raiders stand until
shot. Once fighting, the raiders fire their pistols and swing their cues:
the first run ends with the player dead, the chaos run with every raider
dead and no foe left. No run logged an error; every set has the full
`combat_requirements` (0 placeholders), so no animation is missing.
Containers open and locked ones stay shut (the chaos checks), but the bot
never takes from a container, and looting needs the loot screen: a person
at the keyboard has to check it. `replay-frames` filmed the first run
(`out/fo3b/super-duper-mart/combat/run.gif`, `contact.png`, made on the
first build of the layer), but a plain
`--replay-inputs` of an autoplay log does not repeat the run (only
`--replay-bundle` does, and it writes no frames): from command 1,050 on
the frames hold still with the player unhurt, so they show the approach,
not the fight. A frame option for `--replay-bundle` would be an owner
brief.

**Regressions.** The Prospector Saloon with its slice's defaults
(`out/fnvb/saloon-baseline`) matches `out/baseline-saloon.sha` in all
1,117 files, critter art included (the minimal level is the default
again). Goodsprings with defaults (`all --fnv ... --out out/fnvb/goodsprings`, 84 s) matches `out/baseline-goodsprings-2.sha` in all 6,866 files.

**Frames.** `out/fo3b/super-duper-mart/shots/` (16, all arrived),
`walkthrough/raiders/raiders-classic.gif`, `gallery/items-01.png`,
`gallery/characters/*-strips.png` and `.gif`; the same under
`out/fo3b/megaton-saloon/` (`walkthrough/bar/bar-classic.gif`).

Aid figures: health from restore effects only (magnitude times seconds,
at most 500); an effect that raises maximum health (Buffout's) is left
out. The map stage's key carries a catalogue version (`catalogue-3`)
when loot is on, so a change of these rules rebuilds loot maps only.

**Not done.** Creatures are still not rendered (the Protectron). Weapons
whose damage is their explosion's (grenades, mines) get 1 to 1. Aid items
that raise maximum health are left at their other effects. Lock levels
are not converted (Fallout 2 has none): every lock is locked, none
jammed. A key opens nothing by itself in the engine (a door's script
does that).

**For the next agent (several maps, door links, storeys).** `map.rs`
builds one map through `map_builder::build_map`. The catalogue is made
per scene after slicing (`catalogue::build` takes the scene's
references, the slicer's art and placements and the people) and handed in
`BuildOptions::catalogue`; for `build_layer` it must become one
`LayerOptions::catalogue` over every map's references and placements (item
keys are base form keys, so merging catalogues is a union of `items`,
`placed`, `critters`; draws are seeded per reference, so a container gives
the same contents in any layer). `critter_inputs` and `apply_declutter`
take one scene. `Scene` holds one window, one floor and one
`BuildOptions`; a storey would be one `ElevationInput` per floor band
from `mir::storeys` (the builder's `levels` work). The report writers
(`write_*_section`) take one `BuildReport` and one `Sliced`; the loot
section takes the layer's `LootReport`, which already lists every map.

### Phase B, second half: layers, linked doors, storeys and levels (same day)

On the builder's `build_layer`, `storeys` and `levels` (`crates/map-builder/DESIGN-levels.md`
sections 3 and 4). New code: `mojave_convert::layer` (the layer file, the
plan per map and elevation, the budget split, the report),
`SliceSpec` keys `map`, `storeys`, `storey_floors`, `levels`, `wire`,
`navmesh_coverage`, `sight`, `entrance_sight` (and `entrance` on an
exterior), `--with-interiors`, `estimate` on a layer, per-map features
(`features/<stem>.json`, `MapFeatures::read_for`), `probe walk --elevation`,
walkthroughs that cross exit grids (`Hop::lands`, a re-centre after each
crossing), `scripts/layer-check.sh`, and in `map-slicer` one additive key,
`SlicerConfig::flat_references` (decks of an upper exterior level baked
flat; empty by default, with a test). Formats and keys:
`docs/running.md` ("Layers"); interfaces: `docs/contracts.md` 9.7.

**Layer files** (`slices/fo3/super-duper-mart.layer.toml`, `megaton.layer.toml`,
`springvale.layer.toml`, `slices/goodsprings-linked.layer.toml`); all
`--standalone --jobs 4`, binary `out/bin/mojave-convert-c`:

| | Super-Duper Mart (`out/fo3c/super-duper-mart`) | Megaton (`out/fo3c/megaton`) | Springvale (`out/fo3c/springvale`) | Goodsprings + saloon (`out/fnvc/goodsprings-linked`) |
|---|---|---|---|---|
| maps (elevations) | `sdmart` (1), `sdmartin` (2) | `megaton` (3), `moriarty` (2), `crater` (2), `clinic` (1) | `springva` (1), `bandits` (1) | `goodspri` (1), `saloon` (1) |
| link ends | 4 door, 4 stairs; all both ways | 8 door, 70 stairs (33 transitions of the level split); all both ways; 14 doors keep stubs | 4 door, both ways; 1 stub (the school) | 8 door (the saloon's four doors), both ways; 11 stubs |
| `verify_links` | 22 grids, 4 links, 0 one way | 534 grids, 46 links, 0 one way | 34 grids, 4 links, 0 one way | 249 grids, 24 links, 0 one way |
| walls used of 4,096 (per map, elevation) | 1,329 / 1,431 + 1,291 | 3,002 + 119 + 88 / 112 + 123 / 105 + 110 / 99 | 3,086 / 62 | 3,380 / 263 |
| floor tiles of 4,094 | 3,578 / 469 + 40 | 3,683 + 216 + 91 / 28 + 26 / 17 + 19 / 13 | 4,078 / 10 | 4,041 / 50 |
| people per elevation | 2 / 8 + 2 (the two raiders upstairs placed: they were left out before) | 2 + 1 + 0 / 6 + 0 / 2 + 0 / 4 | 0 / 1 | 3 / 8 |
| walkable, main component share | 33,165, 100 % / 2,126 (99.1 %) + 228 (71.7 %) | 10,084 (34.2 %: three components of 3,451, 3,373, 3,259) + 1,170 (71.5 %) + 490 (100 %) / 116 + 121 / 83 + 96 / 39 | 26,421 / 25 | 30,655 / 207 |
| `all` from an empty `--out` | 3:07, 845 MB | 2:19, 768 MB (critter sets copied from the Phase B Moriarty run, a content-keyed cache) | 2:59, 1.05 GB | 1:30, 779 MB |

Megaton's levels as built: level 0 is the crater floor and the ground
outside the wall, drawing the whole town as one level did; levels 1 and 2
(954 and 396 navmesh hexes) draw 209 and 115 references on their own
origin and bake 51 and 26 decks flat. The entrance is inside the gate
(`entrance` in the layer file, level 0). Moriarty's back door is on its
ground floor in the data; outside it is on level 1, so that link goes from
Megaton elevation 1 to Moriarty's elevation 0 (the balcony is elevation 1,
reached by its stairs). Craterside Supply also splits into two storeys.
23 teleports in Megaton (19 in Goodsprings, 4 in the Mart) named the
worldspace's persistent cell; they are pointed at the cell their partner
door was extracted into (`normalise_teleports`), without which every
interior's door back was one-way.

**Budget.** Proportional shares from the slicer's estimate, interiors
sliced first, the exterior ground level last with the carry-over. The
estimate of a storey counts every reference of the room (it ignores the
cut), so rooms are given more than they use and the rest flows to the
ground level: Megaton's level 0 got 3,340 walls and used 3,002; nothing
overflowed. The Mart's first attempt, sliced in map order with five
"storeys" (navmesh floors at nearly one height), failed with the exterior
given 509 walls; floors are now grouped into storeys by height band.

**Engine acceptance** (`scripts/layer-check.sh`, then walkthroughs; engine
`target/engine-head/target/release/vault-platform`):

- Every map of every layer loads headless (exit 0); `probe walk` per map
  and elevation is in each `engine-check/summary.txt`.
- (a) Doors, by scripted replay (walkthrough frames): Mart car park ->
  front door -> store (arrives by the partner door) -> other front door ->
  car park; Springvale -> bandit house -> out of the back door beside the
  house; Goodsprings -> saloon -> out of another door; Megaton level 1 ->
  Moriarty's back door. Each arrival is the link's arrival hex on the
  other map, beside the partner door. By autoplay: Springvale clean in
  2,000 ticks with 1,359 audited map transitions (the bot back and forth
  through the bandit house's doors) and in a 4,000-tick chaos run with
  3,006; Goodsprings clean with 10, and its chaos run clean with 3,878
  audited transitions and 7 door and container checks held.
- (b) Stairs, by scripted replay: the Mart's stairs up and back down
  (`walkthrough/stairs-both`), Megaton's ramp from the crater floor to
  the level-1 decks (`walkthrough/gate-to-moriarty`); the engine saves and
  reloads the map and places the player on the arrival hex of the other
  elevation each time. By autoplay on Megaton, both runs fail at the first
  audited elevation change: `AuditMismatch at tick 51 (Checks)`, "object
  1335: ObjectRow { pid: 619, hex: None, elevation: 0, ... holder: Some(1)
  } became ObjectRow { ... elevation: 1, ... }" (and object 1337; the
  chaos run at tick 18). The two are items the bot had picked up: the
  live world keeps the elevation they were picked up on while a load seats
  them on the player's. That is the engine (`WorldState::move_object`
  moves the holder only); owner brief
  `docs/vault-engine-briefs/elevation-change-audit.md`, with a synthetic
  reproduction. On the Mart the chaos run went up the second stairs into a
  23-hex island of the upper level and stopped with `StuckLoop` (2,000
  decisions, "Nothing left to do on this map"); its walks onto that
  island's grids back down did not take it down, though the same grids
  work in a replay. I cannot tell whether that is the bot or the layer.
- (c) Mart raiders: on the written map they are on the hostile team with
  their weapons in hand (the report's loot table; the fixture test asserts
  it object by object). In the engine the bot did not fight them this time:
  starting inside the two-storey store it picks up some 60 items without
  walking, aims only at the two raiders upstairs, fails to reach them and
  ends with "Nothing left to do" (clean in 2,000 ticks; `StuckLoop` in the
  chaos run, 0 container checks). In the one-storey Phase B build the same
  bot fought all of them; the difference is the second elevation, and I
  cannot tell from here whether the cause is the layer or the bot. Starting
  on the car park, the plain autoplay never took the front door (27
  actions); the chaos run did.

**Pictures** (`out/fo3c/best/`, `out/fnvc/best/`; classic frames, stationary
camera): `super-duper-mart.png`, `megaton.png`, `springvale.png`,
`goodsprings-linked.png`; GIFs `lot-to-upper-classic.gif` (car park ->
front door -> aisles -> stairs -> upper level), `stairs-both-classic.gif`,
`sdmart-door-both-classic.gif`, `gate-to-moriarty-classic.gif` (gate ->
crater floor -> ramp -> level 1 -> Moriarty's), `springvale-door-both-classic.gif`,
`goodsprings-door-both-classic.gif`; every layer's own `shots-<map>/` and
`walkthrough/` directories.

**What looks wrong, by owner.**
1. `map-builder`/`map-slicer` ("player on roofs"): exterior hexes under a
   building's roof are walkable, so a walker crosses the Mart's store
   "on its roof" (`super-duper-mart/walkthrough/door-both`, frames 10 to
   16) and arrives over the saloon's roof outside its second door in
   Goodsprings (`goodsprings-linked/walkthrough/door-both`, frame 37). The
   Mart layer sets `entrance` in the car park because the window centre is
   inside the store.
2. `map-slicer`: overpass sections render as rows of separate bars in the
   Mart car park (`shots-sdmart/sdmart-05-classic.png`); a white blank
   plane in Megaton (`shots-megaton/megaton-03-classic.png`); level 0 of
   Megaton shows black ground where the land lies far below the town
   (`megaton-02-classic.png`), as in the one-level map.
3. `map-builder`: the Mart's second stairs arrive in a 23-hex island of
   the upper level (no passage: "every way crosses wall art ..."); Megaton
   level 1 keeps islands of 197, 88 and 48 hexes, one holding an exit and
   five stair ends. A tall office-pit wall piece hides the player in the
   store (`shots-sdmartin/sdmartin-05-classic.png`; the builder lists it as
   "walkable furniture?").
4. Engine: the elevation-change audit above (brief written); the bot's
   behaviour on a map of two elevations (not understood).
5. Mine, left: storey grouping keeps the three largest storeys and has no
   spill into further maps (the builder's `layout` can; no target needed
   it); the estimate overcounts storeys; an upper exterior level draws
   only what stands on it, so the town above the walker is not drawn there.

**Regressions and determinism.** With the new keys absent: Goodsprings
(`all --fnv`, 57 s) matches `out/baseline-goodsprings-2.sha` in all 6,866
files and the Prospector Saloon (`out/fnvc/saloon-baseline`, 45 s)
`out/baseline-saloon.sha` in all 1,117. The Springvale layer's map stage
run again into a fresh `--out` (`out/fo3c/springvale-det`) writes the same
12,546 layer files and features; the report differs only in the item-art
cache tally. `tests/slices.rs` checks the eight New Vegas and five
Fallout 3 slices and the four layer files; `tests/mini_mart.rs` builds the
linked two-storey mini mart twice (8 s). Gates: fmt, clippy `-D warnings`,
890 tests, all green.

**Not verified.** Looting by hand (the bot never takes from containers);
the Mart's raiders fighting in the two-storey layer; a full autoplay over
Megaton past the engine's audit; Craterside's and the clinic's interiors
in pictures; a full build with `--with-interiors` (on Springvale its
`estimate` finds the school, three storeys, as `spring02` and the bandit
house as `zspringv`, and splits the budget; no map was built that way).

## 12. Pass 1.5 (stabilization)

What is known so far; written 2026-11-24 while the pass is still open.
Figures below are from the lead's runs of the Phase B layers and were not
re-measured when this section was written.

**Gate.** `scripts/gate.sh` is the local gate (format, clippy
`-D warnings`, tests, the clean-room brief linter, then byte-identity of the
Goodsprings and Prospector Saloon conversions against local baselines); see
the "Gate" section of `docs/running.md` for its inputs, variables and the
`--fast` and `briefs` modes.

**Reach fixes.** The islands of section 11's layer table are now diagnosed
(`map_builder::reach`, `ElevationReport::islands`, `reached`, `closed`), and
the pass added four tools against them: storeys walk their own floors'
triangles (`InteriorBuild::triangles`) instead of the height band;
`navmesh_only` makes the ground outside Megaton's wall not walkable;
`coverage_bridges` joins what a strict coverage rule cuts out of one floor;
and `close_unreached` closes what no walk from the start map's entrance
reaches. The numbers below came with them.

| Elevation | Before | After |
|---|---|---|
| Super-Duper Mart, upper floor | main component 71.7 % of its walkable hexes | 100 % |
| Megaton, level 0 | three components, 34.2 % in the main one | one component |
| Megaton, level 1 | islands of 197, 88 and 48 hexes | fully reached through the stairs, except one 49-hex deck, which is closed; closing it drops one sentry placed on it |

**Rendering.** The white plane in Megaton and the white box of Moriarty's
Saloon were two kinds of shape drawn with ordinary blending: untextured
multiply planes and additive glows (`skip_blend_shapes`), and fill boxes
sampling one texel (`skip_flat_fills`). Both are skipped for Fallout 3
(`docs/contracts.md` section 9).

**Two readable defaults.** With the readable declutter profile: decorative
`MISC` items are pruned (`prune_misc`, at most 1 cap and weight 1, only
unreferenced, non-persistent, no editor id, no linked reference, no script
on the base), and, above the minimal animation level, critters are drawn with
the readable look (a dark 1-pixel rim and the figures tone; `figure_look =
"default"` turns it off). Both in `docs/running.md`.

**The raiders that were never fought (section 11, "Engine acceptance" (c)).**
Diagnosed from real runs. Stairs between elevations are written as walk-over
exit grids. In a real store map a ground item lay exactly on a stair grid hex,
and more along the approach, so the engine's autoplay bot, which collects
items first, stepped onto the grid, was carried upstairs and never fought the
raiders on the ground floor. This was the layer, not the bot. The builder now
keeps every link's grids clear (`map_builder::clearance`: nothing to pick up
on a door's grid or arrival hexes, nor on or within one hex of a stair's grid;
an item there moves to the nearest free walkable hex and the map report says
"moved off link hexes: N"; `verify_clearance` checks the written maps), and
`shots --auto` no longer picks an exit grid as a viewpoint
(`WalkMap::route_to_clear`; an explicit `--at` on a grid is honoured with a
warning). Not yet re-run against the real Mart in the engine.

**Roofs.** The owner's "player on roofs" item (section 11, "What looks wrong",
1) is not a walker standing on a roof: the far side of a building is drawn
under its roof, so a walker behind a tall building is seen through the
engine's see-through circle. Upper exterior levels still draw over
black.

### Caching

The map stage now keeps its renders, grounds and preview PNGs in a
content-hash store (`crates/mojave-convert/src/cache.rs`, `docs/contracts.md`
section 2; flags in `docs/running.md`). Measured on the real Springvale and
Super-Duper Mart layers, release build, `--jobs 2`. Each cell is the whole
`all` run in seconds, with the map stage alone in brackets.

| | Springvale before | Springvale after | Mart before | Mart after |
|---|---|---|---|---|
| cold | 192.7 (77.9) | 166.8 (54.8) | 264.1 (179.5) | 167.0 (82.0) |
| unchanged | 5.5 (0.19) | 5.6 (0.18) | 4.8 (0.19) | 4.8 (0.19) |
| one declutter key | 84.3 (78.9) | 34.1 (21.1) | 181.6 (177.1) | 49.3 (33.5) |
| one builder key | 86.3 (78.5) | 28.8 (16.1) | 181.5 (176.7) | 40.1 (29.3) |
| one map of the layer | 82.8 (77.3) | 22.7 (16.7) | 186.9 (182.3) | 35.2 (30.3) |

Notes:

- 95 % of the slicer's time was rendering, which is what the store keeps.
- Warm output is byte-identical to cold output in every case above.
  `--jobs 1` and `--jobs 4` give identical output too.
- Damaged entries are detected (header and payload checks) and rebuilt.
- Left for later: the builder's 12 to 18 s (a quadratic path-clash check in
  the mod-layer insert), the slicer's non-render work (2.4 to 9 s), the
  gallery stage (3 to 4 s on every run), the critters when cold (66 to 93 s),
  and cache eviction (nothing is ever evicted; deleting the store is safe).

### Simulation layer (Pass 2)

`mojave-convert sim` on the real Mart layer reads
`map-placements.json` (origin `stage`) in about a quarter of a second warm.
It reports 221 placed entries (12 critters, 23 doors, 78 containers, 99
items, 2 terminals, 7 scenery), 332 references left off with their reasons, 3
scripts, 2 packages and 2 terminals.

The owner locked all 15 decisions of `docs/simulation-layer-spec.md` section
13 on 2026-11-23 (the table is in `docs/contracts.md` section 11). The engine
pin bump and the final Megaton autoplay gate wait for the owner's engine
fixes (the elevation-change audit and the camera follow toggle).
