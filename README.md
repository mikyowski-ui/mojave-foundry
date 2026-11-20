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
`critters`, `map`, `gallery`. Every stage skips work whose inputs have not changed. Under
`--out`:

| Path | What |
|---|---|
| `mir/` | the intermediate directory, with `report.md` |
| `sprites/` | one palettized FRM per prop mesh |
| `critters/` | one sprite set per human appearance: stand, walk, placeholders |
| `gallery/` | contact sheets and a composite preview of the map |
| `mods/mojave/` | the mod layer: maps, art, prototypes, lists, `vault-mod.toml` |
| `map-report.md` | art budget, walkability, exits |

`--slice` takes a built-in name (`goodsprings`) or a TOML file; `--config`
takes a calibration TOML (`docs/contracts.md`, section 4); `--backend gpu`
renders through `wgpu` instead of the CPU reference path. `mojave-convert
probe` runs read-only checks of the parsers against real files, and `probe
walk` reports what a walker can reach on a built map.

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
