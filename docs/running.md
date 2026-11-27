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
| Phase B | Items, loot, sides; layers of linked maps, interior storeys, exterior levels | done on Fallout 3 and New Vegas; the Super-Duper Mart, Megaton, Springvale and Goodsprings layers load, link both ways and cross their stairs in the engine; see handoff section 11 |
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
| `map-placements.json` | the placement index the map stage writes beside `map-report.md` (not inside the layer): every placed critter, door, container, item and terminal with its map, elevation, hex, object id and PID as written, and the references of the maps' cells that were left off, each with the stage's reason; the map stage reuses its output only when this file is present; `sim` reads it |
| `cache/` | the map stage's keys (`map.key`, `maps/<stem>.key`) and, unless `--cache-dir` says otherwise, its render, ground and preview store |
| `timings.txt` | what each stage and step took, with cache counts; written by `extract`, `sprites`, `critters`, `map`, `all`, `estimate`, `gallery` and `sim`, each replacing the last (no times reach any converted file) |
| `sim-report.md` | the `sim` stage's counts and cache key (below) |
| `shots/` | engine screenshots, `index.json`, `contact.png` (from `shots`) |

`--timings` prints what each step took, with the cache counts (hits, misses,
damaged entries rebuilt, entries written); the same table is always written
to `<out>/timings.txt` (by the stage commands: `extract`, `sprites`,
`critters`, `map`, `all`, `estimate`, `gallery` and `sim`). `--cache-dir <dir>` moves the map stage's render,
ground and preview store (default `<out>/cache`), so that several `--out`
directories can share one; the stage keys stay in each `<out>/cache`.
Deleting the store is always safe, and nothing is ever evicted. A changed
slice key, declutter threshold, builder option or rebalance value redoes
only the renders and grounds it touches, plus the cheap steps after them
(pruning, cutting, the builder, the write); a run whose inputs have not
changed skips the stage as before. The design is in `docs/contracts.md`
section 2.

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
first conversion showed is in `docs/handoff-pass-1.md` section 11. All five say `loot = true` and
`animations = "combat"` (items, loot, sides; people drawn with their
fights and the weapon they carry); the two rooms also say
`[declutter] profile = "readable"`, and the Super-Duper Mart
`footprints = true` (Moriarty's keeps them off: with them its bar closes
the way to the back). What the Phase B conversion of the two rooms showed
is at the end of handoff section 11.

```
mojave-convert all --standalone --fo3 <Fallout 3> --f2 <Fallout 2> --slice slices/fo3/super-duper-mart.toml --jobs 2 --out <dir>
```

Beside them, three layer files build several of these into one layer
("Layers" below): `super-duper-mart.layer.toml` (the car park and the
two-storey store), `megaton.layer.toml` (the town on three elevations,
Moriarty's, Craterside Supply, the clinic) and `springvale.layer.toml`
(the town and the bandit house); `slices/goodsprings-linked.layer.toml` is
New Vegas's (Goodsprings and the Prospector Saloon).

The decluttered saloon against the plain one (two runs, then one page of
their map previews and the same viewpoints in the engine):

```
mojave-convert all --standalone --slice slices/saloon.toml --fnv <New Vegas> --f2 <Fallout 2> --out <plain>
mojave-convert all --standalone --declutter readable --slice slices/saloon.toml --fnv <New Vegas> --f2 <Fallout 2> --out <readable>
mojave-convert before-after --before <plain> --after <readable> --out saloon-pairs.png
mojave-convert shots --engine <vault-platform> --f2 <Fallout 2> --out <readable> --at <hex> --at <hex> --classic
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
| `prune_misc`, `prune_misc_max_value`, `prune_misc_max_weight` | on, 1, 1 | prune decorative miscellaneous items (`MISC`) worth at most that many caps and weighing at most that much; only in the readable profile |

A miscellaneous item is pruned as decorative (skip reason "decorative
item", counted in `## Declutter`) only when nothing marks it as part of the
game: it is unreferenced (`referenced_refs`), not persistent, has no editor
id and no linked reference, and its base has no script, and its base is
within both thresholds. Weapons, ammunition, aid, keys, armour, notes, books
and every `MISC` above the thresholds stay protected by kind;
`prune_misc = false` turns the rule off.

`[tone.floor]`, `[tone.props]` and `[tone.figures]` take `gain`, `gamma`,
`saturation` and `contrast` over the preset's curves. The slicer tones the
baked floor and overlays (floor) and every upright render (props); the
terrain outside does not take the floor tone yet, nor do the critter
sprites the figures tone. `map-report.md` gains `## Declutter` (the
profile and its source, pruned by rule, kept and pruned by kind, protected
by reason, the cutaway, the floor before and after) and `## Readability`
(`BuildReport::readability_summary`, written on every run).

### Loot, sides and the rebalance tables

`loot = true` in a slice, or `--loot on` (on `map`, `all`, `estimate`;
`--loot off` wins over the slice), gives the map real items: one item
prototype per weapon, armour, ammunition, aid item, key and other item
the window places, its containers hold or its people carry, each with its
record's name, a short generated description, a ground sprite and an
inventory icon; containers filled (leveled lists drawn once per
container), locked containers and doors, keys; and its people armed,
armoured and on a side. Off (the default for every slice that does not
say so), nothing changes. The Fallout 3 slices have it on.

```
mojave-convert all --standalone --fo3 <Fallout 3> --f2 <Fallout 2> --slice slices/fo3/super-duper-mart.toml --jobs 2 --out <dir>
mojave-convert map --standalone --loot on --rebalance my-tables.toml ...
```

The figures come from the rebalance tables: `rebalance/fo3.toml` and
`rebalance/fnv.toml`, full copies of the built-in defaults with a header
saying how to tune. A run reads `--rebalance <file>`, else
`rebalance/<game>.toml` beside the slice file or in a directory above it
(the working directory for a built-in slice), else the built-in tables;
a file only needs the keys it changes. Besides `emit`'s tables (damage,
action points, ranges, armour, ammunition, aid, fighters, weapon classes,
calibres, skills) the file has the converter's own:

| Table | What |
|---|---|
| `[convert.weapons]` | `by_animation`: a weapon record's animation type to its class; `energy_skill`; `[[convert.weapons.rules]]` refine by editor-id pattern, animation type and skill (knives, swords and axes are blades, spears, shotguns, SMGs, flamers; plasma, electric, pulse and explosive damage) |
| `[convert.hostility]` | `player_faction`, `hostile_aggression` (2), `enemy_aggression` (1), `hostile_patterns` (used only when an actor has no AI data), `friendly_patterns`, `hostile_team` (66), `hostile_packet` ("Merc Raider") and `hostile_packet_number` (136), `friendly_team` and `friendly_packet` (empty: the builder's defaults) |
| `[convert.loot]` | `level` (1: the player level leveled lists are drawn at), `max_carried` (12), `max_contents` (24) |

A person is hostile when its AI data (after its templates) says very
aggressive or frenzied, unless one of its factions and the player's are
friends; or aggressive with a faction that is the player's enemy. Hostile
people share the hostile team and AI packet; the rest stay on the
builder's peaceful team. Fallout 2 critters attack the player only when
attacked (or scripted); in the engine's autoplay the bot attacks every
unscripted critter of another team.

`map-report.md` gains `## Loot` (item prototypes by kind, containers
filled, locks, keys, people with their side, the reason, packet, team,
weapon, armour and kit, and every item with its figures) and
`## Furniture`; `## Characters` gives each sprite set's files and bytes
and the `critters.lst` rows used. `gallery/items-NN.png` shows every
item's ground sprite with its inventory icon. `footprints = true` in a
slice blocks the hexes under furniture (tables, counters, shelves,
freezers) found by the slicer.

### Layers: several maps, storeys and levels

A **layer file** (`<name>.layer.toml`, given to `--slice`) builds several
slices into one layer and one `--out`: an exterior and the interiors its
doors lead to, the doors linked both ways (an exit grid on each side
arriving on a walkable hex beside the partner door), doors leading outside
the build keeping their stub maps, one art budget for the whole layer.

```toml
name = "super-duper-mart"     # the layer; the report's title
start = "sdmart"              # the start map's stem (default: the first map)
with_interiors = false        # also every interior the exterior's doors lead to
close_unreached = true        # default; false keeps every walkable component, reached or not
presence_copies = false       # default; true: a hidden copy of a named actor on each map its packages lead to

[all_maps]                    # slice keys laid over every map's slice
loot = true
wire = true
[all_maps.declutter]
profile = "readable"

[[maps]]
slice = "super-duper-mart-exterior.toml"   # relative to the layer file, or a built-in name
map = "sdmart"                              # any slice key: overrides the file's

[[maps]]
slice = "super-duper-mart.toml"
map = "sdmartin"
storeys = "auto"

[[maps]]
interior = "MegatonClinic"    # no slice file: the first exterior's keys, as an interior
```

`close_unreached` (on by default, in a layer file and on a one-map build;
`close_unreached = false` in the layer file, or in a slice for a one-map
build, turns it off) closes every walkable component that no walk from the
start map's entrance reaches through the layer's links (stepping on an exit
grid goes on from its arrival hex): the hexes become not walkable, no
entrance or arrival stands on them, and each is listed as "closed as
unreached". A component a person would stand on is kept: it is judged from
each critter's own source position before anything is closed, and an
isolated deck or island that holds a placed person stays walkable and is
listed as an "occupied perch" (size, a representative hex, who stands
there, for example "occupied perch: 49 hexes at hex H, holds Stockholm
(not reached from the start)"). A person is never dropped by closing: each
elevation says "critters lost to closing: none" (the build lists any it
would lose). The reach is reported either way. Link hexes keep their
keep-clear rule on a perch as anywhere else.

`presence_copies = true` is a layer file key, not a slice key (spec 7.6): a
named actor whose AI packages take it to another map of the layer gets one
hidden copy there, on the walkable hex nearest the package's location, marked
`presence_of` in the placement index (its reference is `<original>@<map stem>`).
A package location is understood when it is a reference, a cell of a map of the
layer, or the actor's linked reference. It is off by default, and it is part of
the layer's cache key. `enable_groups` also works inside a layer (as an
`[all_maps]` key or a map's own). All three change the written maps only when
set; with them off the output is byte for byte what it was.

The trigger volumes (a primitive on an activator) and the key items of locks
are written to `map-placements.json` (`docs/contracts.md` 11.2) whenever the
map stage runs: each trigger with its hex, shape and a covering hex radius, and
each lock with `key_pid` when the layer holds the key's prototype. That needs
a MIR of schema 3; an older extract directory is rebuilt by the next run.

Every map must be of one game, one plugin list and one worldspace: extract,
sprites and critters run once over their union (the union of the cell
ranges, the deepest `interior_depth`; above the minimal animation level the
critter stage renders only the people the maps place). Map stems must be
distinct (`map` names one; an exterior's stub maps take its first four
letters, so two exteriors need different heads). An interior of a layer
splits its storeys unless it says `storeys = "one"`, and needs no
`--standalone` of its own (the layer's lists are shared; the layers checked
in are run standalone). `--with-interiors` on `map`, `all` and `estimate`
turns an exterior slice into such a layer: every interior its doors (in the
window) lead to, each with the exterior's keys, `place_disabled = true`,
`storeys = "auto"` and a stem from its editor id (`MegatonClinic` in
`MegatonWorld` is `clinic`).

| Slice key | Default | What |
|---|---|---|
| `map` | the name's first eight characters | the map file stem |
| `storeys` | `one` (`auto` in a layer) | interior: every storey of the navmesh its own elevation (`map_builder::storeys::split_storeys`; navmesh floors within one band of a larger floor's height are one storey; the three largest storeys are kept), the stairs and ramps between them paired exit grids |
| `storey_floors` | the three largest | interior, with `storeys = "auto"`: which storeys (0 the lowest) become elevations 0, 1, 2, in order |
| `levels` | `one` | exterior: `auto` splits the navmesh into levels that never lie over one another (`map_builder::levels::split_levels`), up to three elevations, ramps between them paired exit grids |
| `entrance` | (none) | on an exterior too: the source point `[x, y]` the entrance is sought near (`BuildOptions::entrance_near`); with `levels = "auto"` and no `entrance`, the window's first door leading to another worldspace |
| `wire` | off | wire props (shopping carts) drawn solid (`map_slicer::WireRule`); the Fallout 3 readable preset then keeps its carts |
| `navmesh_coverage` | `tolerance` | `centre` or `share:F`; `share:0.5` is recommended indoors with footprints |
| `sight` | `any` | `visible`: `## Readability` counts a figure covered only when more than a quarter of its box is under art drawn after it |
| `entrance_sight` | `any` | the entrance rule's sight |
| `navmesh_only` | off | exterior: cells without a navmesh are not walkable when the window has one elsewhere (`BuildOptions::navmesh_only`); used for the ground outside Megaton's wall |
| `vertex_color_floor` | 0 | exterior: the darkest a land vertex colour may make the ground, 0 to 1 (`TerrainOptions::vertex_color_floor`) |
| `coverage_bridges` | on | with a strict `navmesh_coverage`, joins the islands it cuts out of one floor through hexes the tolerance rule covers, at most 8 per join (`BuildOptions::coverage_bridges`) |
| `close_unreached` | on | one-map builds (a layer takes the layer file's key): close the walkable components no walk from the entrance reaches, except those a person stands on (`BuildOptions::close_unreached`) |
| `enable_groups` | off | Pass 2 (spec 8.3): a reference of an enable-parent group (a parent, or a child) that starts disabled is placed with the hidden flag instead of being left off, so a script can show it later; members follow their group's initial state, not their own flag. A member the slicer would bake into the ground or skip stays off and is listed. Hidden objects block nothing and take no part in walkability or the reach. The report gains `## Enable groups`; the placement index lists every one (`hidden`, `group`) |
| `hostile_packet` | the rebalance table's | with `loot` on: the AI packet number hostile people take, a packet of the base `ai.txt` (another number stops the stage); the placement index marks the people it applied to as `hostile` |

These keys are read on a one-map run as well, each only when present; a
slice with `storeys = "auto"` or `levels = "auto"` is built as a layer of
one map. Two exceptions are on by default: `coverage_bridges` (it acts only
under a strict `navmesh_coverage`) and `close_unreached` (above), for both
games and for one-map builds too; `= false` in a slice (or a layer file for
`close_unreached`) restores the old bytes. Because of that default,
Goodsprings differs from the baselines recorded before 2026-11-24 wherever it
had a walkable island no walk from the entrance reaches; the Prospector
Saloon (one interior component) is unchanged unless the results below say
otherwise.

**Storeys.** One slicer run and one terrain run per storey: the slicer's
interior mode at the storey's floor, its ceiling cut never above the next
storey (10 units under it), the floor bake as that elevation's ground;
people go to the storey whose band holds their height (outside every
kept band they are "on another level", or clamped with `clamp_levels`); a
door's placement is kept on the storey nearest its height only.

**Levels.** Level 0 is the exterior run of a one-level map, with only
level 0's navmesh walkable, and it still draws the whole town (the decks
above stand over the ground as on one level). A level above has no
terrain: its references (those whose highest surface at or below their
origin is on it, `LevelSplit::reference_level`; doors by
`level_of_point`) are drawn on their own origin, and the references that
carry its walking surface (the smallest placed footprint holding each of
its navmesh triangles, two or more triangles) are baked flat as its floor
(`SlicerConfig::flat_references`). The rest of an upper level is blank.

**Budget.** Every map and elevation is estimated (`map_slicer::estimate`)
and given its share of the walls, scenery and items lists in proportion
(largest remainder); interiors are sliced first, then the levels above an
exterior's ground, its ground level last, each run passing on what it
leaves unused, so the ground level's budget ladder takes the squeeze. Floor
tiles are shared in proportion to each elevation's floor (the exterior's
ground level counts 10,000 squares), smallest first, with carry-over.
`estimate` on a layer prints the table.

**Output.** `map-report.md` opens with `## Layer`: the maps (number,
elevations, entrance, walkable hexes and people per elevation, links,
stubs), every link with its destination and whether it leads back, the
`verify_links` result, the stubs, the budget per map and elevation (demand,
share, use, tiles); then each map with its storeys or levels and per
elevation its reach, slicer, ground and open-hex sections. Features are in
`features/<stem>.json` per map and `map-features.json` for the start map;
previews `gallery/map-<stem>.png` and `map-<stem>-e<N>.png`. `shots
--map <file>` and `walkthrough --map <file>` read that map's features;
`probe walk <layer> <map> --elevation N` walks any elevation (from the
stairs' first arrival when it is not the entrance's). A `walkthrough
--through` hex that is an exit grid ends that leg on the grid: the walk
goes on from its arrival hex on the grid's map and elevation, the camera
re-centred.

```
mojave-convert all --standalone --fo3 <Fallout 3> --f2 <Fallout 2> --slice slices/fo3/super-duper-mart.layer.toml --jobs 4 --out <dir>
mojave-convert all --standalone --fo3 <Fallout 3> --f2 <Fallout 2> --slice slices/fo3/megaton.layer.toml --jobs 4 --out <dir>
mojave-convert all --standalone --fo3 <Fallout 3> --f2 <Fallout 2> --slice slices/fo3/springvale.layer.toml --jobs 4 --out <dir>
mojave-convert all --standalone --fnv <New Vegas> --f2 <Fallout 2> --slice slices/goodsprings-linked.layer.toml --jobs 4 --out <dir>
mojave-convert all --standalone --with-interiors --fo3 <Fallout 3> --f2 <Fallout 2> --slice slices/fo3/springvale.toml --out <dir>
scripts/layer-check.sh <vault-platform> <Fallout 2> <dir> 4000 <mojave-convert>
```

`scripts/layer-check.sh` loads every map headless, walks every elevation
(`probe walk`), and runs a 2,000-tick autoplay and a 4,000-tick
`--chaos-doors-c4` autoplay from the start map; the summary is
`<dir>/engine-check/summary.txt`.

Each elevation's section of the report carries, besides its components,
passages and islands left:

- `reached from the layer's start through links: R of N`: the walkable hexes
  a walk from the start map's entrance reaches (stepping on an exit grid
  continues from its arrival hex), of the elevation's planned walkable
  hexes, with the share;
- `island <size> hexes at <hex>: ...`: each walkable component other than
  the main one, saying whether the ground joins it to the main component
  ("the ground joins it to the main component", with `in the way:` the hexes
  that are not walkable on the shortest way and what keeps them so: a wall
  strip, scenery, a footprint, relief), or keeps it apart ("the ground keeps
  it apart", or "... under this coverage rule, the tolerance rule would join
  it"), whether it is "reached through links" or "NOT reached through
  links", and what it holds (grids, arrivals, critters);
- `closed as unreached: <size> hexes at <hex>, holding ...`, with
  `close_unreached`;
- `occupied perch: <size> hexes at hex <hex>, holds <names> (not reached
  from the start)`: an unreached component kept walkable because a person
  stands there; `critters lost to closing: none` (or the names) after it;
  a one-map build prints the same lines, with the reached count, under
  `## Reach`;
- `moved off link hexes: N`, a line of the map's `## Notes` (prefixed
  `elevation <e>: ` above elevation 0): the ground items and containers the
  builder moved off the exit grids of links (below).

**Ground loot and exit grids.** Links between elevations and maps are
walk-over exit grids, so a bot that collects items first steps onto a grid
lying under an item and is carried away. In a layer with links, no ground
item, no container that does not block its hex and no critter's standing hex
is on an exit grid hex of a link (a door's grids or a stair's), on a link's
arrival hex, or within one hex of a stair's grid hex
(`map_builder::clearance::STAIRS_KEEP_CLEAR_RADIUS`; a door keeps only its
grid hexes and arrival hexes, `DOOR_KEEP_CLEAR_RADIUS` 0, since door mats
legitimately have clutter nearby). An item placed there moves to the nearest
free walkable hex outside that set, in the same walkable component when it
has room, ties to the lowest hex; critters are placed clear of it. A
container that blocks its hex stays (it cannot stand on a grid, which is
walkable). A map without links is written as before. `verify_clearance`
checks the written maps and fails the build otherwise.

**Known limits.** Stairs are walk-over exit grids, not stair objects: a
walker steps onto the grid and the map reloads on the other elevation. A
building's far side is drawn under its roof, so a walker behind a tall
building is seen through the engine's see-through circle (not standing on
the roof). Upper exterior levels draw over black: an elevation above 0 has
floor only under its decks.

### Critter stage options

| Flag (on `critters`, `all`) | Slice key | Default | What |
|---|---|---|---|
| `--animations minimal\|combat\|full` | `animations` | `minimal` | a stand, a walk and stills (the first wave's bytes); combat adds every unarmed animation the source has and the armed set of the weapon carried; full adds pistol and rifle sets for everyone |
| `--deaths ragdoll\|game` | `deaths` | `ragdoll` | falls and corpses from the physics sampler, or the game's knock-downs |
| `--stride exact\|matched\|whole_hexes` | `stride` | `exact` | walk travel against the grid |
| `--figure-look default\|readable` | `figure_look` | `readable` with the readable declutter profile and an animation level above `minimal`, else `default` | the readable look: a dark 1-pixel rim and the figures tone; `[tone.figures]` is laid over it; `default` (or `--figure-look default`) turns it off |
| `--only <editor id>` | | all | render only these actors |

A figure look needs `combat` or `full`; the readable declutter profile turns
it on there unless the slice or the flag says `default`. The Fallout 3 slices say
`animations = "combat"`; the New Vegas ones keep `minimal`, so their
layers stay byte-identical. A combat set is about 3.5 to 4.6 MB and 21 to
24 animation files of six directions. `--rebalance` is on every
subcommand: it classifies the weapons the critter stage draws in hand.

### Engine checks of a fight

`replay-frames` films a recorded engine run (an autoplay bundle's
`inputs.jsonl`) by replaying growing prefixes of it, keeping the last
frame of each, and writes the frames, a contact page and a GIF; `before-after`
pairs two runs' map previews on one page.

```
E=target/engine-head/target/release/vault-platform
$E --autoplay-headless --game-profile fallout2 --master-dat $F2/master.dat --critter-dat $F2/critter.dat \
   --mod-list load_order.toml --preview-map maps/super-du.map --ticks 4000 --chaos-doors-c4 --autoplay-output engine-check/chaos
mojave-convert replay-frames --engine $E --f2 $F2 --out <dir> --log <dir>/engine-check/autoplay/inputs.jsonl --from 300 --every 30
mojave-convert before-after --before <dir without the pass> --after <dir with it> --out pairs.png
```

To play a converted map in a `vault-engine` build:

```
scripts/play.sh <vault-platform binary> <Fallout 2 install> <the --out directory>
```

### Sim

```
mojave-convert sim --out <dir>
```

`sim` reads an already-built `--out` directory: the MIR (`mir/`), the layer
(`mods/mojave`) and the placement index `map-placements.json`, which the map
stage writes beside `map-report.md` (not inside the layer). It needs no game
install, renders nothing, slices and builds no map, and changes no file of
the layer. It checks that the three belong together (the same game, every
indexed map registered in the layer's manifest), counts what the source has
for the placed references (scripts, packages, dialogue infos, topics and
quests, terminals and notes, and how many placed critters and objects have a
script, packages or dialogue), prints those counts and the stage cache key
(`sim-1:` and a hash of the game, slice, plugins, MIR rows, base files, the
map stage's key from `<out>/cache/map.key`, every map's bytes and the index), and writes `sim-report.md` beside
`map-report.md` (no time, host or path: a second run on the same output is
byte-identical). `--timings` prints what the stage took (the table is always written to
`<out>/timings.txt`). On the real Super-Duper Mart layer the stage reads the
index in about a quarter of a second warm. A directory without
`map-placements.json` is read from its built maps instead, with a warning
(no references, bases or enable parents). No scripts are compiled yet; this
is the scaffold of Pass 2 (`docs/simulation-layer-spec.md`). Two parts of the
cache key are placeholders today: the transpiler version and the `[sim]` table
of `foundry.toml`, which the scaffold does not read. The interfaces are in
`docs/contracts.md` section 11.

## Building

```
cargo test --workspace
```

`vault-formats` is a git dependency pinned by revision. The pin is now
`2e632c2` (it was `3adeac9` before wave S1; `.cargo/config.toml.example`
already names it). To resolve it from a local `vault-engine` checkout instead
of the network, copy `.cargo/config.toml.example` to `.cargo/config.toml` and
set the path.

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

## Gate

`scripts/gate.sh` runs the local gate in order and stops at the first failure:
`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, the clean-room
brief linter, and then byte-identity checks of the Goodsprings and Prospector
Saloon conversions against local baselines (`out/baseline-goodsprings-3.sha`,
`out/baseline-saloon.sha`; the file counts must match too). Give the installs
with `FNV_DIR` and `F2_DIR` (or `--fnv` and `--f2`); `GATE_BASELINES`,
`GATE_SCRATCH`, `GATE_BIN` and `GATE_JOBS` override the defaults. Without the
installs or baselines the last step prints a SKIP line and the gate still
passes, unless `GATE_REQUIRE_REAL=1`; `--fast` skips it on purpose.
`scripts/gate.sh briefs` runs only the brief linter, and
`scripts/gate.sh briefs --self-test` checks the linter itself.
