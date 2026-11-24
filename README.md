# mojave-foundry

> *What if New Vegas had shipped in 1998?*

An offline pipeline that melts down your own Fallout 3 or Fallout: New
Vegas install and recasts it as a Fallout 2 format mod layer for
[`vault-engine`]. The 3D world goes in one end; 256-colour isometric
sprites, hex maps and prototype tables come out the other, and the Lone
Wanderer and the Courier get to walk Megaton and the Strip the way the
Chosen One would have seen them. Both games go through the same pipeline;
it tells them apart by the master plugin the install holds.

[`vault-engine`]: https://github.com/mikyowski-ui/vault-engine

It is a proof of concept, not a game: towns, landmarks and rooms you can
walk through, with the people who live there standing in them.

## Ground rules

- **Bring your own games.** You need your own install of Fallout 3 or
  Fallout: New Vegas, and of Fallout 2. The pipeline reads them and ships
  none of them.
- **Personal and non-commercial.** Converted output contains material
  derived from both games. It stays on the machine that owns them: it is
  not shared, uploaded or committed.
- **No game data in this repository.** Only code, coordinates and notes.

## How it works

The foundry runs as a line of stations. Each one reads what the last one
wrote, and skips work whose inputs have not changed.

1. **Extract.** Open the game's archives and plugin, and pull out one
   slice of the world: a block of outdoor cells, or a single room. Meshes,
   textures, placed objects, walkable areas and the people in them are
   written to a neutral intermediate format.
2. **Render sprites.** Every prop, wall and building is rendered from its
   3D mesh at Fallout 2's fixed camera angle, then reduced to Fallout 2's
   256-colour palette.
3. **Pose people.** Each resident is dressed from their inventory, posed on
   the game's own skeleton with its own animations, and rendered standing
   and walking in the six directions a hex grid needs.
4. **Build the map.** The ground becomes floor tiles. Buildings are sliced
   into wall pieces that sort correctly as you walk behind them. The
   original walkable areas decide which hexes are open, and everything is
   fitted into Fallout 2's limits on art and map size. Indoors, floors are
   baked from the room's own pieces and the ceiling is cut away.
5. **Emit the layer.** Maps, art, prototypes and lists are written as a mod
   layer the engine loads on top of Fallout 2.
6. **Prove it.** The layer is loaded in the engine with no window, walked
   by its autoplay bot, and photographed.

## Where to look next

- [`docs/running.md`](docs/running.md): status, commands, slice files and
  build notes.
- [`docs/contracts.md`](docs/contracts.md): the interfaces the crates share.
- [`docs/handoff-pass-1.md`](docs/handoff-pass-1.md): what has been built,
  what each area needed, and what is known to be wrong.
