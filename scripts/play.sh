#!/usr/bin/env bash
# Play a converted map in vault-engine, in a window, as a new Fallout 2 game.
#
#   scripts/play.sh <engine-binary> <fallout2-dir> <out-dir> [map] [engine flags...]
#
#   <engine-binary>  a vault-platform executable
#   <fallout2-dir>   a Fallout 2 install (holds master.dat and critter.dat)
#   <out-dir>        what `mojave-convert all --out` wrote (holds mods/mojave)
#   [map]            a map of the layer, with or without `.map` (default: the
#                    first map the layer registers)
#
# The game starts the normal way (`--game`), so the Fallout 2 new-game state
# is installed: the Pip-Boy shows the layer's quests.txt and holodisk.txt and
# the player's script has its local variables. The layer is mounted over the
# Fallout 2 archives with the enhanced preset (Alt+F cycles the game speed,
# Alt+V the viewport). Controls: any key skips the two start movies; N is New
# Game; Left and Right choose a premade character, T takes it; any key skips
# the hand-off movie; the game then starts on [map]. Saves go to
# <out-dir>/saves, so the install's own saves are never touched.
#
# The start map is the engine's `StartingMap` key, which it reads under sfall
# mode (`--sfall`, which the script passes) from a ddraw.ini in a mod layer.
# The script writes that layer to <out-dir>/mods/play-start.
#
# Anything after [map] is passed to the engine, for example `--preset vanilla`.
# `--preview` (before or after [map]) is the old behaviour: the map as a plain
# preview, with no new-game state (no sfall mode, no menus), the player on the
# map at once.
set -euo pipefail

if [ "$#" -lt 3 ]; then
    sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi

engine=$1
f2=$(realpath "$2")
out=$(realpath "$3")
shift 3

layer="$out/mods/mojave"
[ -x "$engine" ] || { echo "error: $engine is not an executable" >&2; exit 1; }
[ -f "$layer/vault-mod.toml" ] || { echo "error: no mod layer at $layer; run mojave-convert first" >&2; exit 1; }

find_archive() {
    local name
    for name in "$1" "$(echo "$1" | tr '[:lower:]' '[:upper:]')"; do
        if [ -f "$f2/$name" ]; then
            echo "$f2/$name"
            return 0
        fi
    done
    echo "error: no $1 in $f2" >&2
    return 1
}
master=$(find_archive master.dat)
critter=$(find_archive critter.dat)

if [ "$#" -gt 0 ] && [ "${1#--}" = "$1" ]; then
    map=$1
    shift
else
    map=$(grep -m1 '^file = ' "$layer/vault-mod.toml" | cut -d'"' -f2)
fi
stem=${map%.map}
map=$stem.map
[ -f "$layer/maps/$map" ] || { echo "error: no maps/$map in the layer" >&2; exit 1; }

# `--preview` is this script's flag: take it out of what the engine gets.
preview=0
rest=()
for flag in "$@"; do
    if [ "$flag" = "--preview" ]; then preview=1; else rest+=("$flag"); fi
done
set -- ${rest[@]+"${rest[@]}"}

mkdir -p "$out/saves"

preset=(--preset enhanced)
for flag in "$@"; do
    case $flag in --preset | --preset=*) preset=() ;; esac
done

if [ "$preview" = 1 ]; then
    printf 'layers = ["mods/mojave"]\n' > "$out/load_order.toml"
    exec "$engine" \
        --game-profile fallout2 \
        "${preset[@]}" \
        --master-dat "$master" \
        --critter-dat "$critter" \
        --mod-list "$out/load_order.toml" \
        --preview-map "maps/$map" \
        --save-dir "$out/saves" \
        "$@"
fi

# A layer of its own, after the converter's, carrying only the start map.
start="$out/mods/play-start"
mkdir -p "$start"
printf 'name = "Play start"\nid = "play-start"\napi = 1\nversion = "1"\n' > "$start/vault-mod.toml"
printf '[Misc]\nStartingMap=%s\n' "$stem" > "$start/ddraw.ini"
printf 'layers = ["mods/mojave", "mods/play-start"]\n' > "$out/load_order.toml"

exec "$engine" \
    --game \
    --game-profile fallout2 \
    --sfall \
    "${preset[@]}" \
    --master-dat "$master" \
    --critter-dat "$critter" \
    --mod-list "$out/load_order.toml" \
    --save-dir "$out/saves" \
    "$@"
