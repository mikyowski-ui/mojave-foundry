#!/usr/bin/env bash
# Play a converted map in vault-engine, in a window.
#
#   scripts/play.sh <engine-binary> <fallout2-dir> <out-dir> [map] [engine flags...]
#
#   <engine-binary>  a vault-platform executable
#   <fallout2-dir>   a Fallout 2 install (holds master.dat and critter.dat)
#   <out-dir>        what `mojave-convert all --out` wrote (holds mods/mojave)
#   [map]            a file under the layer's maps/ (default: the first map
#                    the layer registers)
#
# The layer is mounted over the Fallout 2 archives with the enhanced preset
# (Alt+F cycles the game speed, Alt+V the viewport). Saves go to
# <out-dir>/saves, so the install's own saves are never touched. Anything
# after [map] is passed to the engine, for example `--preset vanilla`.
set -euo pipefail

if [ "$#" -lt 3 ]; then
    sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
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
[ -f "$layer/maps/$map" ] || { echo "error: no maps/$map in the layer" >&2; exit 1; }

printf 'layers = ["mods/mojave"]\n' > "$out/load_order.toml"
mkdir -p "$out/saves"

preset=(--preset enhanced)
for flag in "$@"; do
    case $flag in --preset | --preset=*) preset=() ;; esac
done

exec "$engine" \
    --game-profile fallout2 \
    "${preset[@]}" \
    --master-dat "$master" \
    --critter-dat "$critter" \
    --mod-list "$out/load_order.toml" \
    --preview-map "maps/$map" \
    --save-dir "$out/saves" \
    "$@"
