# Running mojave-foundry

Status, commands, slice keys and build notes. The overview is in the
[README](../README.md).

## Status

| Milestone | Scope | State |
|---|---|---|
| M0 | Workspace, policies, contracts | done |
| M1 | Extract to MIR: archives, plugins, static meshes, textures, report | done, verified on a real install of each game |
| M2 | Sprite foundry and Fallout 2 emission adapters | done, verified on a real install |
| M3 | Goodsprings exterior as a native Fallout 2 map | done; loads, draws and walks in the engine headless, exits work both ways; see the handoff for what it found |
| M4 | Skinned characters, faces, ragdoll deaths | first wave done: townsfolk stand and idle on the map, posed from the game's skeleton and animations; no faces, creatures or death art yet |
| M5 | Script transpiler and the Goodsprings quests | not started |

## Running

```
mojave-convert all --game-dir <Fallout 3 or New Vegas install> --f2 <Fallout 2 install> --out <directory outside git>
mojave-convert all --fo3 <Fallout 3 install> --f2 <Fallout 2 install> --out <dir>      # the same, game fixed
mojave-convert all --fnv <New Vegas install> --f2 <Fallout 2 install> --out <dir>      # as before
```

### Which game

One pipeline converts both games. `--game-dir` takes either install (or
its `Data` directory) and reads the game from the master plugin `Data`
holds (`Fallout3.esm` or `FalloutNV.esm`), confirmed by that plugin's
header version (0.94 for Fallout 3, 1.34 for New Vegas). `--game fo3|fnv`
names the game instead; `--fo3 <dir>` and `--fnv <dir>` are `--game-dir`
with the game fixed. Without `--slice` a run converts the game's built-in
slice: `goodsprings` for New Vegas, `springvale` for Fallout 3. A slice
file may say `game = "fo3"` or `"fnv"`; without it its game is the one
whose master its `plugins` list starts with. Any two of these that
disagree (a Fallout 3 install under `--fnv`, a New Vegas slice with
`--fo3`, `game = "fnv"` with `plugins = ["Fallout3.esm"]`, a master whose
header is the other game's) stop the run before extract with a message
naming both. The game is recorded in `mir/manifest.json` and `report.md`,
in `map-report.md` (its second line) and in `map-features.json` (`game`).
From Fallout 3 on it is part of the sprite and map stages' cache keys;
New Vegas keys are unchanged.

The map stage starts the slicer from a per-game pattern profile
(`mojave_convert::map::slicer_defaults`): New Vegas's is the slicer's
default (Mojave patterns, byte-identical), Fallout 3's drops the `nv`
name prefix, puts rubble and canyon pieces in relief, flattens litter and
puddles, keeps elevated highways and overpasses upright, and draws dead
trees and stumps as single sprites.

Stages are also subcommands of their own: `extract`, `sprites`,
`critters`, `map`, `gallery`. Every stage skips work whose inputs have not changed. Two
more subcommands read a run's output: `estimate` and `shots` (below), and
`contact` lays any PNG files out on one labelled page
(`mojave-convert contact --out page.png --columns 4 Label=a.png b.png`). Under
`--out`:

| Path | What |
|---|---|
| `mir/` | the intermediate directory, with `report.md` |
| `sprites/` | one palettized FRM per prop mesh |
| `critters/` | one sprite set per human appearance: stand, walk, placeholders |
| `gallery/` | contact sheets and a composite preview of the map |
| `mods/mojave/` | the mod layer: maps, art, prototypes, lists, `vault-mod.toml` |
| `map-report.md` | the interior (on an interior slice), art budget and lists, budget ladder, landmark pieces, ground, open hexes, walkability, entrance, components, passages, islands, exits |
| `map-features.json` | the map's largest landmarks and buildings (on an interior demo also the room and its people), for `shots --auto` |
| `shots/` | engine screenshots, `index.json`, `contact.png` (from `shots`) |

`--slice` takes a built-in name (`goodsprings`, `springvale`) or a TOML file; `--config`
takes a calibration TOML (`docs/contracts.md`, section 4); `--backend gpu`
renders through `wgpu` instead of the CPU reference path. `mojave-convert
probe` runs read-only checks of the parsers against real files, and `probe
walk` reports what a walker can reach on a built map. `probe esm <plugin>`
prints the plugin's game and every typed decoder's tally (both tables);
`--cells <text>` breaks the placed references of the cells whose editor id
contains the text down by the record type of their base:

```
mojave-convert probe esm "<Fallout 3>/Data/Fallout3.esm" --cells superdupermart
```

### Demonstration maps

`slices/` holds slice files (cell ranges and a map centre, no game data)
for four dense areas: `strip.toml`, `primm.toml`, `helios.toml` and
`nellis.toml`. What the second conversion of all four showed, with
numbers and the open defects, is in `docs/handoff-pass-1.md` section 9. They are meant for **standalone mode**:

```
mojave-convert all --standalone --slice slices/helios.toml --fnv <New Vegas> --f2 <Fallout 2> --out <dir>
```

`--standalone` (on `map` and `all`) makes the layer replace Fallout 2's
wall and floor-tile lists instead of extending them, so the map gets about
4,090 entries of each instead of about 2,400 walls and under 1,000 tiles
in a stock install. Stock
Fallout 2 maps draw the wrong walls and floors while such a layer is
mounted; use it for demonstrations only. A standalone run also leaves
every hex the game's navmesh lets an actor stand on open inside a
building's bounding footprint (a run without the flag does so only near
doors, to save wall entries), so porches, arcades and the ground between
close buildings stay walkable, and it takes in buildings that stand
outside the window but reach into it. Without the flag nothing changes.

Before the long run, `estimate` gives the art budget's pre-check in under a
second, after `extract`: per list the demand against the free entries, the
largest contributors, landmarks, and references by class. It renders
nothing.

```
mojave-convert estimate --standalone --slice slices/helios.toml --fnv <New Vegas> --f2 <Fallout 2> --out <dir>
```

`shots` has the engine itself take screenshots of a converted map,
headless, through its replay mode. It walks the player from the entry to
each viewpoint in hops of 15 steps (fewer when a hop would leave the
frame) along a route of the written map. By default it captures the
**classic** frame: the engine's enhanced view on a 640x480 surface at zoom
1.0, 640x380 pixels of map without the interface bar, from a camera that
stands still and is re-centred only when the next hop would leave its
inner box (`--margin`, 0.15 of each side). `--wide` adds the 1920x1080
widescreen view at the wide zoom; `--game-view` the 640x480 game view with
its interface, which the engine re-centres on the player at every step;
`--follow-camera` lets the camera follow the player (alone, it captures
the game view and the wide view, as `shots` did before); `--dest` writes
elsewhere. `walkthrough` records one walk (`--through HEX...`, or
`--scene`) as frames and an animated GIF.

```
mojave-convert shots --engine <vault-platform binary> --f2 <Fallout 2> --out <dir> --auto 4 --classic --wide
mojave-convert shots --engine <vault-platform binary> --f2 <Fallout 2> --out <dir> --at 20100 --at 18342
```

`--auto N` picks up to N viewpoints: the entry, then the reachable hexes
nearest the map's four largest landmarks, then its other largest
landmarks and buildings (`map-features.json`, written by the map stage),
at least 30 steps apart and each model once before any model twice. `--at` adds a hex of
your own; an unreachable one is replaced by the nearest reachable hex.
Output goes to `<dir>/shots/`: `<map>-NN-classic.png`, `<map>-NN-wide.png` and `<map>-NN-game.png` (the views asked for),
`index.json` (hex, view, file, and whether the player arrived, judged by
comparing frames since the engine reports no position), `contact.png`, and
the replay logs and engine logs in `work/`.

### Fallout 3 slices

`slices/fo3/` holds the Fallout 3 slices, each with `game = "fo3"` and
`plugins = ["Fallout3.esm"]`: `springvale.toml` (the built-in default),
`megaton.toml` (the town, in its own worldspace `MegatonWorld`),
`super-duper-mart-exterior.toml`, and two rooms, `super-duper-mart.toml`
and `megaton-saloon.toml` (Moriarty's Saloon, cut at 120). What their
first conversion showed is in `docs/handoff-pass-1.md` section 11.

```
mojave-convert all --standalone --fo3 <Fallout 3> --f2 <Fallout 2> --slice slices/fo3/super-duper-mart.toml --jobs 2 --out <dir>
```

### Interior demo rooms

An interior slice builds one interior cell (of either game) as a standalone,
walkable demo map with its people standing in it: its own layer and its
own `--out`, one storey, doors drawn but leading nowhere, no interior
lighting, no creatures. `slices/` has four New Vegas rooms: `saloon.toml` (the Prospector
Saloon, built and checked), `vikki-vance.toml`, `ultra-luxe.toml` and
`tops.toml`; `slices/fo3/` two Fallout 3 ones. They always need `--standalone`:

```
mojave-convert all --standalone --slice slices/saloon.toml --jobs 3 --fnv <New Vegas> --f2 <Fallout 2> --out <dir>
mojave-convert shots --engine <vault-platform binary> --f2 <Fallout 2> --out <dir> --auto 8
mojave-convert probe walk <dir>/mods/mojave saloon.map --hop 15
```

The slice keeps `worldspace`, `x` and `y` (the exterior cells whose doors
lead to the room, so extract reaches it; `interior_depth` at least 1) and
adds:

| Key | Default | What |
|---|---|---|
| `interior` | (none: an exterior slice) | the cell's editor id |
| `floor_z` | the largest navmesh storey's floor | the floor height, room coordinates |
| `map_center` | that floor's navmesh centre | the window centre, room coordinates |
| `entrance` | the room's first teleport door (lowest form key) | `[x, y]` the player starts nearest |
| `place_disabled` | `false` | also place people the data starts disabled |
| `clamp_levels` | `false` | people off the floor level stand on the nearest walkable hex instead of being left out |
| `cut_height` | 210 | the ceiling cut above the floor (the saloon uses 150) |
| `floor_band` | `[-16, 32]` | heights above the floor baked into floor tiles |
| `hang_clearance` | 160 | a reference reaching over the cut whose lowest point is higher hangs and is left out |
| `walk_band` | `[-40, 100]` | heights around the floor whose navmesh is walkable |

`map-report.md` then opens with `## Interior` (the floor and storeys, the
cut, the floor bake, the doors not converted, and every person placed or
left out with the reason), and `map-features.json` names the room and
ranks its people, so `shots --auto` aims at the entrance, then at each
person (named people first), then at the largest walls; the automatic
viewpoints may lie 5 steps apart (`--spread`), and the wide view is
captured at zoom 0.75 (`--wide-zoom`). A person whose actor has fewer than
1 hit point is left out as dead in the data.

### Declutter and tone

A slice's `[declutter]` and `[tone]` tables drive the slicer's declutter
and readability pass (`map_slicer::DeclutterProfile`): protection of
interactive and quest-referenced things, clutter, pile and decal pruning,
overhang pruning and a lower shell cut indoors, the foreground cutaway,
class tones and outlines. It is off by default for both games, so every
existing slice keeps its bytes; `profile = "readable"` in the table, or
`--declutter readable` on `map`, `all` and `estimate` (which wins over the
slice), turns it on from the game's readable preset
(`mojave_convert::declutter::readable_for`; Fallout 3 adds its paper and
litter heaps and its shopping carts, wire meshes that read as scribbles,
to the clutter patterns). With it on, the references that
quests, scripts, conditions, links, enable parents and packages point at
(`mir::Records::referenced_refs`) are protected, and they, every
reference of a protected kind (items count even with `protect_items = false`) and every teleport door are handed to the
builder as interactive (`BuildOptions::interactive`: never a flat floor
piece). Every key below overrides one setting of the preset; any of them
with the pass off is an error.

| `[declutter]` key | Readable default | What |
|---|---|---|
| `profile` | `off` | `off` or `readable` |
| `protect_items` | `true` | `false` takes the item kinds (bottles, cans, glasses, ammunition, junk) off the protected kinds, so the clutter rules may drop small ones |
| `protected_kinds` | containers, doors, terminals, activators, items | replaces the list |
| `protect_referenced` | `true` | protect the quest and script references |
| `clutter`, `clutter_max_volume`, `clutter_max_hexes`, `clutter_max_height`, `clutter_patterns` | on, 15,000, 0.5, 36, papers and debris | the micro-clutter rule; `clutter_patterns` adds to the patterns |
| `density` | on | thinning of dense piles |
| `decals`, `decal_fade` | `fade`, 0.25 | `keep`, `drop` or `fade` at that opacity |
| `overhang`, `overhang_clearance` | on, 80 | beams, ducts and fixtures that high above the floor, indoors |
| `shell_cut`, `shell_cut_height` | on, 120 | the lower cut of room shells, indoors |
| `cutaway`, `kerb_height`, `cutaway_exteriors` | on, 35, `false` | the foreground cutaway; outdoors only when asked |
| `outline`, `outline_factor` | on, 0.45 | the dark outline of interactive sprites |
| `measure_baseline` | `true` | also measure the window without the pass |

`[tone.floor]`, `[tone.props]` and `[tone.figures]` take `gain`, `gamma`,
`saturation` and `contrast` over the preset's curves. The slicer tones the
baked floor and overlays (floor) and every upright render (props); the
terrain outside does not take the floor tone yet, nor do the critter
sprites the figures tone. `map-report.md` gains `## Declutter` (the
profile and its source, pruned by rule, kept and pruned by kind, protected
by reason, the cutaway, the floor before and after) and `## Readability`
(`BuildReport::readability_summary`, written on every run).

To play a converted map in a `vault-engine` build:

```
scripts/play.sh <vault-platform binary> <Fallout 2 install> <the --out directory>
```

## Building

```
cargo test --workspace
```

`vault-formats` is a git dependency pinned by revision. To resolve it from a
local `vault-engine` checkout instead of the network, copy
`.cargo/config.toml.example` to `.cargo/config.toml` and set the path.

## Layout

| Crate | Role |
|---|---|
| `fnv-bsa` | BSA version 104 archive reader |
| `fnv-esm` | Plugin record walker and typed extraction |
| `nif` | Static mesh parser and glTF binary export |
| `dds` | Texture decoder |
| `mir` | The intermediate representation directory |
| `foundry-render` | Oblique parallel sprite renderer |
| `quantize` | Palette quantizer |
| `emit` | Fallout 2 file writers and the mod layer |
| `foundry-gallery` | Contact sheets |
| `map-terrain`, `map-slicer`, `map-builder` | The map pipeline |
| `mojave-convert` | The command line |
| `ragdoll`, `transpile` | Placeholders for M4 and M5 |

Shared interfaces are in `docs/contracts.md`. Requests to `vault-engine` are
in `docs/vault-engine-briefs/`.
