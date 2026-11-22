# mojave-foundry

An offline pipeline that converts your own Fallout: New Vegas install into a
Fallout 2 format mod layer for [`vault-engine`]: sprites rendered from the
3D meshes, hex maps built from the exterior cells, and the tables that tie
them together.

[`vault-engine`]: https://github.com/mikyowski-ui/vault-engine

## Rules of use

- **Bring your own games.** You need a legitimately owned install of
  Fallout: New Vegas and of Fallout 2. The pipeline reads both and ships
  neither. Fallout 2 supplies the palette, interface art, fonts and sound
  effects that the output relies on.
- **Non-commercial.** This is a personal, non-commercial tool.
- **No sharing of output.** Converted output contains material derived from
  both games. Do not distribute it, upload it or commit it. Keep it on the
  machine that owns the games.
- **No game data in this repository.** See `AGENTS.md`.

## Status

| Milestone | Scope | State |
|---|---|---|
| M0 | Workspace, policies, contracts | done |
| M1 | Extract to MIR: archives, plugins, static meshes, textures, report | done, verified on a real install |
| M2 | Sprite foundry and Fallout 2 emission adapters | done, verified on a real install |
| M3 | Goodsprings exterior as a native Fallout 2 map | done; loads, draws and walks in the engine headless, exits work both ways; see the handoff for what it found |
| M4 | Skinned characters, faces, ragdoll deaths | first wave done: townsfolk stand and idle on the map, posed from the game's skeleton and animations; no faces, creatures or death art yet |
| M5 | Script transpiler and the Goodsprings quests | not started |

## Running

```
mojave-convert all --fnv <New Vegas install> --f2 <Fallout 2 install> --out <directory outside git>
```

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

`--slice` takes a built-in name (`goodsprings`) or a TOML file; `--config`
takes a calibration TOML (`docs/contracts.md`, section 4); `--backend gpu`
renders through `wgpu` instead of the CPU reference path. `mojave-convert
probe` runs read-only checks of the parsers against real files, and `probe
walk` reports what a walker can reach on a built map.

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
each viewpoint in hops of 15 steps along a route of the written map, and
captures the 640x480 game view and the 1920x1080 widescreen view at zoom
0.5:

```
mojave-convert shots --engine <vault-platform binary> --f2 <Fallout 2> --out <dir> --auto 4
mojave-convert shots --engine <vault-platform binary> --f2 <Fallout 2> --out <dir> --at 20100 --at 18342
```

`--auto N` picks up to N viewpoints: the entry, then the reachable hexes
nearest the map's four largest landmarks, then its other largest
landmarks and buildings (`map-features.json`, written by the map stage),
at least 30 steps apart and each model once before any model twice. `--at` adds a hex of
your own; an unreachable one is replaced by the nearest reachable hex.
Output goes to `<dir>/shots/`: `<map>-NN-game.png` and `<map>-NN-wide.png`,
`index.json` (hex, view, file, and whether the player arrived, judged by
comparing frames since the engine reports no position), `contact.png`, and
the replay logs and engine logs in `work/`.

### Interior demo rooms

An interior slice builds one New Vegas interior cell as a standalone,
walkable demo map with its people standing in it: its own layer and its
own `--out`, one storey, doors drawn but leading nowhere, no interior
lighting, no creatures. `slices/` has four: `saloon.toml` (the Prospector
Saloon, built and checked), `vikki-vance.toml`, `ultra-luxe.toml` and
`tops.toml`. They always need `--standalone`:

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
