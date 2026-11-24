#!/usr/bin/env bash
# Engine checks of a converted layer, headless.
#
#   scripts/layer-check.sh <engine-binary> <fallout2-dir> <out-dir> [ticks] [converter]
#
#   <engine-binary>  a vault-platform executable
#   <fallout2-dir>   a Fallout 2 install (holds master.dat and critter.dat)
#   <out-dir>        what `mojave-convert all --out` wrote (holds mods/mojave)
#   [ticks]          autoplay ticks (default 4000)
#   [converter]      a mojave-convert executable for `probe walk`
#                    (default: mojave-convert on the PATH)
#
# For every map the layer registers: a headless load (a replay of a few
# ticks over it) and `probe walk` of every elevation it writes. Then a
# 2,000-tick autoplay and a `--chaos-doors-c4` autoplay of [ticks] from the
# start map (the first map registered). Everything goes to
# <out-dir>/engine-check/; the summary to engine-check/summary.txt.
set -uo pipefail

if [ "$#" -lt 3 ]; then
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi

engine=$(realpath "$1")
f2=$(realpath "$2")
out=$(realpath "$3")
ticks=${4:-4000}
convert=${5:-mojave-convert}
layer="$out/mods/mojave"
check="$out/engine-check"
mkdir -p "$check"
[ -f "$layer/vault-mod.toml" ] || { echo "error: no mod layer at $layer" >&2; exit 1; }
printf 'layers = ["mods/mojave"]\n' > "$out/load_order.toml"
summary="$check/summary.txt"
: > "$summary"

# The registered maps, in manifest order: `file` lines of [[maps]].
mapfiles=$(grep -E '^file *= *"' "$layer/vault-mod.toml" | sed -E 's/^file *= *"([^"]+)".*/\1/')
start=$(printf '%s\n' "$mapfiles" | head -n 1)

idle="$check/idle.jsonl"
{
    printf '{"command_id":1,"kind":"surface","width":640,"height":480}\n'
    for i in $(seq 2 31); do
        printf '{"command_id":%d,"kind":"tick","now_ms":%d}\n' "$i" $(( (i - 1) * 16 ))
    done
} > "$idle"

for map in $mapfiles; do
    stem=${map%.map}
    ( cd "$out" && "$engine" --replay-inputs "$idle" --game-profile fallout2 \
        --master-dat "$f2/master.dat" --critter-dat "$f2/critter.dat" \
        --mod-list load_order.toml --preview-map "maps/$map" ) \
        > "$check/load-$stem.out" 2> "$check/load-$stem.err"
    status=$?
    printf 'load %s: exit %s; %s\n' "$map" "$status" "$(tail -n 1 "$check/load-$stem.out")" >> "$summary"
    # Stub maps (`<head>_NNN.map`) are empty: nothing to walk.
    case "$stem" in *_[0-9][0-9][0-9]) continue ;; esac
    for e in 0 1 2; do
        if "$convert" probe walk "$layer" "$map" --elevation "$e" --hop 15 > "$check/walk-$stem-e$e.txt" 2>&1; then
            if grep -q ' 0 objects on elevation' "$check/walk-$stem-e$e.txt"; then
                rm -f "$check/walk-$stem-e$e.txt"
                continue
            fi
            printf 'probe walk %s elevation %s: %s\n' "$map" "$e" \
                "$(sed -n '2p' "$check/walk-$stem-e$e.txt")" >> "$summary"
        fi
    done
done

run_autoplay() {
    name=$1
    shift
    rm -rf "$check/$name"
    ( cd "$out" && "$engine" --autoplay-headless --game-profile fallout2 \
        --master-dat "$f2/master.dat" --critter-dat "$f2/critter.dat" \
        --mod-list load_order.toml --preview-map "maps/$start" \
        --autoplay-output "engine-check/$name" "$@" ) \
        > "$check/$name.out" 2> "$check/$name.err"
    status=$?
    printf '\nautoplay %s (exit %s):\n' "$name" "$status" >> "$summary"
    if [ -f "$check/$name/report.txt" ]; then
        sed 's/^/  /' "$check/$name/report.txt" >> "$summary"
    else
        tail -n 5 "$check/$name.err" | sed 's/^/  /' >> "$summary"
    fi
}

run_autoplay autoplay --ticks 2000
run_autoplay chaos --ticks "$ticks" --chaos-doors-c4
cat "$summary"
