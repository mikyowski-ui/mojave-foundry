#!/usr/bin/env bash
# Readability A/B: builds the variants of scripts/ab/variants.conf one after
# another, checks them in the engine, photographs the same world spots in
# each, and writes the contact sheets and a summary.
#
#   scripts/readability-ab.sh [--only NAME]... [--skip-engine] [--base DIR]
#                             [--force] [--dry-run] [--out-root DIR]
#
#   --only NAME     build only this variant (repeatable); default: all
#   --skip-engine   no engine check and no frames (build, sheets of what
#                   exists, summary only)
#   --base DIR      the warm output directory to copy instead of the
#                   variant's group's (variants.conf `base=`; default
#                   out/fix3/megaton and out/fix3/mart); applies to every
#                   selected variant, so use it with --only
#   --force         rebuild a variant whose directory holds a finished marker
#   --dry-run       print every command, run nothing
#   --out-root DIR  where the variants go (default out/readability-ab); the
#                   sheets and summary.md are written there too
#
# Per variant (never two at once):
#   1. copy the warm base into <out-root>/<variant>. The cache/ directory is
#      hard-linked (cp -al): the converter's cache store writes each entry to
#      a temporary name and renames it into place and deletes a bad entry, and
#      never rewrites one in place (crates/mojave-convert/src/cache.rs), so a
#      link can never change the base's copy. Everything else (mir, with its
#      database, sprites, critters, indexes) is copied for real (cp -r),
#      because those files are rewritten in place. Old engine-check, saves and
#      shots are left out.
#   2. mojave-convert all --standalone --fo3 .. --f2 .. --slice <layer>
#      --jobs 2 --timings --out <dir> --readability K=V ...; wall time into
#      <dir>/ab-built, the log into <dir>/ab-build.log.
#   3. unless --skip-engine: scripts/layer-check.sh (every map loads, bot
#      runs), then scripts/ab/take-shots.py (the sheet frames).
#   4. the finished marker <dir>/ab-done. A variant with one is skipped
#      unless --force (one made with --skip-engine is finished only for a
#      later --skip-engine run).
# Then <out-root>/<group>-ab-sheet.png per group (megaton-ab-sheet.png,
# mart-ab-sheet.png) and <out-root>/summary.md (FID budgets, timings, loads,
# bot results, play commands).
#
# Install paths: the environment, else the git-ignored gate.env at the repo
# root (one KEY=value per line, no quoting needed):
#   FO3_DIR   a Fallout 3 install (required, except with --dry-run)
#   F2_DIR    a Fallout 2 install (required)
#   ENGINE    a vault-platform executable (required unless --skip-engine)
#   CARGO_HOME, FNV_DIR  read by the other scripts; ignored here
# AB_BIN      the mojave-convert to use (default target/debug/mojave-convert);
#             it must know `--readability` for the variants that pass it.
#
# The yaw angles of the comparison are BEST_YAW and ALT_YAW at the top of
# scripts/ab/variants.conf; a variant that needs an empty one is refused.
set -euo pipefail

ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AB="$ROOT/scripts/ab"

only=()
skip_engine=0
base_override=
force=0
dry=0
out_root=out/readability-ab

while [ "$#" -gt 0 ]; do
    case "$1" in
        --only) [ "$#" -ge 2 ] || { echo "error: --only needs a name" >&2; exit 2; }; only+=("$2"); shift 2 ;;
        --skip-engine) skip_engine=1; shift ;;
        --base) [ "$#" -ge 2 ] || { echo "error: --base needs a directory" >&2; exit 2; }; base_override=$2; shift 2 ;;
        --force) force=1; shift ;;
        --dry-run) dry=1; shift ;;
        --out-root) [ "$#" -ge 2 ] || { echo "error: --out-root needs a directory" >&2; exit 2; }; out_root=$2; shift 2 ;;
        -h | --help) sed -n '2,52p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown option $1 (try --help)" >&2; exit 2 ;;
    esac
done

# Local defaults from gate.env; a variable already set in the environment wins.
if [ -f "$ROOT/gate.env" ]; then
    while IFS='=' read -r key value || [ -n "$key" ]; do
        case "$key" in
            FO3_DIR | F2_DIR | ENGINE | AB_BIN)
                if [ -z "${!key:-}" ]; then
                    value="${value%\"}"
                    value="${value#\"}"
                    export "$key=$value"
                fi
                ;;
        esac
    done < "$ROOT/gate.env"
fi

abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$ROOT" "$1" ;; esac; }

AB_BIN=$(abs "${AB_BIN:-target/debug/mojave-convert}")
out_root=$(abs "$out_root")

if [ "$dry" = 0 ]; then
    [ -n "${FO3_DIR:-}" ] || { echo "error: FO3_DIR is not set: put a FO3_DIR=<Fallout 3 install> line in gate.env (git-ignored) or export it" >&2; exit 1; }
    [ -d "$FO3_DIR" ] || { echo "error: FO3_DIR $FO3_DIR is not a directory" >&2; exit 1; }
    [ -n "${F2_DIR:-}" ] && [ -d "$F2_DIR" ] || { echo "error: F2_DIR is not set or not a directory (gate.env or the environment)" >&2; exit 1; }
    [ -x "$AB_BIN" ] || { echo "error: $AB_BIN is not an executable: build mojave-convert or set AB_BIN" >&2; exit 1; }
    if [ "$skip_engine" = 0 ]; then
        [ -n "${ENGINE:-}" ] || { echo "error: ENGINE is not set: put an ENGINE=<vault-platform executable> line in gate.env (git-ignored), export it, or pass --skip-engine" >&2; exit 1; }
        [ -x "$(abs "$ENGINE")" ] || { echo "error: ENGINE $ENGINE is not an executable" >&2; exit 1; }
    fi
fi
FO3_DIR=${FO3_DIR:-<FO3_DIR>}
F2_DIR=${F2_DIR:-<F2_DIR>}
if [ -n "${ENGINE:-}" ]; then ENGINE=$(abs "$ENGINE"); else ENGINE="<ENGINE>"; fi

# Print and run, or only print.
run() {
    if [ "$dry" = 1 ]; then
        printf '+'
        printf ' %q' "$@"
        printf '\n'
    else
        "$@"
    fi
}
say() { printf '== %s\n' "$*"; }

variants=$(python3 -I "$AB/abconf.py" variants)

selected() {
    [ "${#only[@]}" -eq 0 ] && return 0
    local name
    for name in "${only[@]}"; do [ "$name" = "$1" ] && return 0; done
    return 1
}
for name in ${only[@]+"${only[@]}"}; do
    printf '%s\n' "$variants" | cut -f1 | grep -qx -- "$name" || { echo "error: no variant named $name in variants.conf" >&2; exit 2; }
done

if [ "$dry" = 0 ]; then
    mkdir -p "$out_root"
    mkdir "$out_root/.lock" 2> /dev/null || { echo "error: $out_root/.lock exists: another run is going (remove it if not)" >&2; exit 1; }
    trap 'rmdir "$out_root/.lock" 2> /dev/null || true' EXIT
fi

now() { date +%s.%N; }

groups_touched=()
failed=()
while IFS=$'\t' read -r name group layer base missing pairs; do
    [ -n "$name" ] || continue
    selected "$name" || continue
    dir="$out_root/$name"
    [ -n "$base_override" ] && base=$base_override
    base=$(abs "$base")
    layer_file=$(abs "$layer")
    groups_touched+=("$group")

    # Done already?
    if [ -f "$dir/ab-done" ] && [ "$force" = 0 ]; then
        if [ "$skip_engine" = 1 ] || grep -q 'stages=.*engine' "$dir/ab-done"; then
            say "$name: finished (ab-done), skipped; --force rebuilds"
            continue
        fi
    fi
    if [ "$missing" != "-" ]; then
        if [ "$dry" = 1 ]; then
            echo "warning: $name needs $missing, which is empty in scripts/ab/variants.conf (a real run refuses)" >&2
        else
            echo "error: $name needs $missing, which is empty in scripts/ab/variants.conf: fill it in" >&2
            exit 1
        fi
    fi

    readability=()
    if [ "$pairs" != "-" ]; then
        for pair in $pairs; do readability+=(--readability "$pair"); done
        if [ "$dry" = 0 ] && ! "$AB_BIN" all --help 2> /dev/null | grep -q -- '--readability'; then
            echo "error: $AB_BIN has no --readability flag yet (variant $name): build a converter that has it, or set AB_BIN" >&2
            exit 1
        fi
    fi

    say "$name ($group): $dir"

    # 1. The warm copy. A half-made copy (no ab-copied) is thrown away.
    if [ "$force" = 1 ] || [ ! -f "$dir/ab-copied" ]; then
        [ -d "$base" ] || { echo "error: base $base is not a directory: run the plain conversion there first" >&2; exit 1; }
        run rm -rf "$dir"
        run mkdir -p "$dir"
        for entry in "$base"/*; do
            item=$(basename "$entry")
            case "$item" in
                engine-check | saves | shots | walkthrough | ab-* | load_order.toml) continue ;;
                cache) run cp -al "$entry" "$dir/$item" ;;
                *) run cp -r "$entry" "$dir/$item" ;;
            esac
        done
        run touch "$dir/ab-copied"
        rm -f "$dir/ab-built" "$dir/ab-done" 2> /dev/null || true
    fi

    # 2. The conversion.
    if [ ! -f "$dir/ab-built" ] || [ "$force" = 1 ]; then
        convert=("$AB_BIN" all --standalone --fo3 "$FO3_DIR" --f2 "$F2_DIR" --slice "$layer_file" --jobs 2 --timings --out "$dir" ${readability[@]+"${readability[@]}"})
        if [ "$dry" = 1 ]; then
            printf '+'
            printf ' %q' "${convert[@]}"
            printf ' > %q 2>&1\n' "$dir/ab-build.log"
        else
            start=$(now)
            if "${convert[@]}" > "$dir/ab-build.log" 2>&1; then
                end=$(now)
                wall=$(awk -v a="$start" -v b="$end" 'BEGIN { printf "%.1f", b - a }')
                {
                    echo "wall_s=$wall"
                    echo "readability=${pairs}"
                    printf 'command=%s\n' "${convert[*]}"
                } > "$dir/ab-built"
                echo "built in $wall s"
            else
                echo "error: conversion of $name failed; see $dir/ab-build.log" >&2
                tail -n 15 "$dir/ab-build.log" >&2
                failed+=("$name")
                continue
            fi
        fi
    fi

    # 3. Engine checks and frames.
    stages=build
    if [ "$skip_engine" = 0 ]; then
        run "$ROOT/scripts/layer-check.sh" "$ENGINE" "$F2_DIR" "$dir" 2000 "$AB_BIN" > /dev/null || true
        run python3 -I "$AB/take-shots.py" --variant "$name" --dir "$dir" --converter "$AB_BIN" --engine "$ENGINE" --f2 "$F2_DIR" || failed+=("$name (frames)")
        stages=build,engine,shots
    fi

    # 4. The marker; ab-done holds the wall time the summary reads.
    if [ "$dry" = 0 ]; then
        { cat "$dir/ab-built"; echo "stages=$stages"; } > "$dir/ab-done"
    fi
done <<< "$variants"

# Sheets and summary, over every variant that has frames.
for group in $(printf '%s\n' ${groups_touched[@]+"${groups_touched[@]}"} | sort -u); do
    run python3 -I "$AB/make-sheet.py" --group "$group" --root "$out_root" --converter "$AB_BIN" --output "$out_root/$group-ab-sheet.png" || failed+=("sheet $group")
done
if [ "$dry" = 0 ]; then
    python3 -I "$AB/collect.py" --root "$out_root" --engine "$ENGINE" --f2 "$F2_DIR"
else
    printf '+ python3 -I %q --root %q --engine %q --f2 %q\n' "$AB/collect.py" "$out_root" "$ENGINE" "$F2_DIR"
fi

if [ "${#failed[@]}" -gt 0 ]; then
    echo "failed: ${failed[*]}" >&2
    exit 1
fi
echo "done: $out_root"
