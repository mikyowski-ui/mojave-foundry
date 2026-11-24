#!/usr/bin/env bash
# The local gate: formatting, lints, tests, the clean-room brief linter and,
# when the owner's installs and baselines are present, byte-identity checks.
#
#   scripts/gate.sh [--fast] [--fnv DIR] [--f2 DIR]
#   scripts/gate.sh briefs [--self-test]     # only the brief linter
#
# Environment:
#   FNV_DIR, F2_DIR     install directories (or --fnv / --f2)
#   GATE_BASELINES      directory of baseline-*.sha files (default: out)
#   GATE_SCRATCH        scratch directory, wiped per run (default: out/gate)
#   GATE_BIN            use this mojave-convert binary instead of building one
#   GATE_JOBS           cargo -j value (default: 4)
#   GATE_REQUIRE_REAL=1 a skipped byte-identity step is a failure
#   CARGO_HOME          honoured if set, never forced
#
# --fast skips the byte-identity step on purpose.
set -euo pipefail

ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRIEFS_DIR="docs/vault-engine-briefs"
JOBS="${GATE_JOBS:-4}"

# ---------------------------------------------------------------------------
# Brief linter. Briefs are handed to the engine's builders, so they must not
# name engine source files, commit hashes, or game and converter specifics.
# ---------------------------------------------------------------------------

# lint_pattern LABEL REASON ERE FILE...   prints "file:line: reason" per hit.
lint_pattern() {
  local flags="$1" reason="$2" pattern="$3"
  shift 3
  local f hits=0 out
  for f in "$@"; do
    # grep exits 1 on no match; that is the good case.
    out="$(grep -nE${flags} -e "$pattern" -- "$f" || true)"
    if [ -n "$out" ]; then
      while IFS= read -r line; do
        printf '%s:%s: %s\n' "$f" "${line%%:*}" "$reason" >&2
        hits=$((hits + 1))
      done <<<"$out"
    fi
  done
  return "$hits"
}

# lint_files FILE...   returns the number of violations (capped at 255 in $?).
lint_files() {
  local total=0 rc
  run() { rc=0; lint_pattern "$@" || rc=$?; total=$((total + rc)); }

  run "" "engine source path (crates/vault-)" 'crates/vault-' "$@"
  run "" "engine source path (src/ segment)" '(^|[^A-Za-z0-9_-])src/' "$@"
  run "" "source file token (.rs)" '[A-Za-z0-9_]\.rs([^A-Za-z0-9_]|$)' "$@"
  run "i" "40-character hex string" '(^|[^0-9a-f])[0-9a-f]{40}([^0-9a-f]|$)' "$@"
  run "iw" "game-specific or converter-specific name" \
    'Fallout 3|Fallout: New Vegas|New Vegas|FO3|FNV|Gamebryo|Bethesda|fnv_decomp|mojave|Megaton|Springvale|Super-Duper|Goodsprings|Moriarty|NIF|ESM|BSA' "$@"
  run "iw" "decompilation project name" 'fallout2-ce|fallout1-re' "$@"

  LINT_TOTAL="$total"
  [ "$total" -eq 0 ]
}

lint_briefs() {
  local files=() f
  for f in "$ROOT/$BRIEFS_DIR"/*.md; do
    [ -e "$f" ] && files+=("${f#"$ROOT"/}")
  done
  if [ "${#files[@]}" -eq 0 ]; then
    echo "brief linter: no briefs found under $BRIEFS_DIR" >&2
    return 1
  fi
  (builtin cd "$ROOT" && lint_files "${files[@]}")
}

lint_self_test() {
  local tmp bad good failures=0 name text
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/brief-lint.XXXXXX")"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  builtin cd "$tmp"

  # Each case: file name and a single line that must be rejected.
  local -a names=(path src rs hash fo3 fnv newvegas goodsprings nif esm bsa mojave gamebryo ce)
  local -a texts=(
    'see crates/vault-formats for the parser'
    'lives in a src/ directory'
    'edit frm_writer.rs next'
    'pinned at 0123456789abcdef0123456789ABCDEF01234567 exactly'
    'Ported from Fallout 3 content'
    'the FNV plugin'
    'the New Vegas install'
    'tested on Goodsprings'
    'a NIF mesh'
    'an esm record'
    'a BSA archive'
    'the mojave converter'
    'Gamebryo scene graph'
    'compare fallout2-ce behaviour'
  )
  local i
  for i in "${!names[@]}"; do
    name="${names[$i]}.md"
    printf '%s\n' "${texts[$i]}" >"$name"
    if lint_files "$name" >/dev/null 2>&1; then
      echo "self-test FAIL: line was accepted: ${texts[$i]}" >&2
      failures=$((failures + 1))
    fi
  done

  # Lines that must pass, including look-alikes.
  local -a goods=(
    'A writer in vault_core::world reads vault-mod.toml and sfall words.'
    'Revision 3adeac9 cites SPEC-023 section 3.17 and the fonts (not a hash).'
    'Names such as infinite, signif and nifty are not whole-word matches.'
    'A [Must-match] rule for the .frm layout; the 4,096-entry limit.'
  )
  for i in "${!goods[@]}"; do
    good="good$i.md"
    printf '%s\n' "${goods[$i]}" >"$good"
    if ! lint_files "$good" >/dev/null 2>&1; then
      echo "self-test FAIL: good line was rejected: ${goods[$i]}" >&2
      lint_files "$good" >&2 || true
      failures=$((failures + 1))
    fi
  done

  # The reported location must be file:line.
  printf 'fine line\nthe FNV plugin\n' >loc.md
  text="$(lint_files loc.md 2>&1 || true)"
  case "$text" in
    loc.md:2:*) ;;
    *)
      echo "self-test FAIL: expected a loc.md:2: report, got: $text" >&2
      failures=$((failures + 1))
      ;;
  esac

  if [ "$failures" -ne 0 ]; then
    echo "brief linter self-test: $failures failure(s)" >&2
    return 1
  fi
  echo "brief linter self-test: ok (${#names[@]} bad lines rejected, ${#goods[@]} good lines accepted)"
}

if [ "${1:-}" = "briefs" ]; then
  shift
  if [ "${1:-}" = "--self-test" ]; then
    lint_self_test
    exit $?
  fi
  if lint_briefs; then
    echo "brief linter: ok"
    exit 0
  fi
  echo "brief linter: violations found (see file:line lines above)" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# The gate.
# ---------------------------------------------------------------------------

FAST=0
FNV="${FNV_DIR:-}"
F2="${F2_DIR:-}"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --fast) FAST=1 ;;
    --fnv)
      FNV="${2:?--fnv needs a directory}"
      shift
      ;;
    --f2)
      F2="${2:?--f2 needs a directory}"
      shift
      ;;
    -h | --help)
      sed -n '2,20p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      echo "gate: unknown argument: $1" >&2
      exit 2
      ;;
  esac
  shift
done

builtin cd "$ROOT"

abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$ROOT" "$1" ;; esac; }
BASELINES="$(abs "${GATE_BASELINES:-out}")"
SCRATCH="$(abs "${GATE_SCRATCH:-out/gate}")"

STEP_NAMES=()
STEP_RESULTS=()
record() { STEP_NAMES+=("$1"); STEP_RESULTS+=("$2"); }

summary() {
  echo
  echo "gate summary"
  echo "------------"
  local i
  for i in "${!STEP_NAMES[@]}"; do
    printf '%-6s %s\n' "${STEP_RESULTS[$i]}" "${STEP_NAMES[$i]}"
  done
}

fail() {
  record "$1" FAIL
  echo "GATE FAIL: $1" >&2
  summary
  exit 1
}

# step NAME CMD...: run a command; stop the gate on the first failure.
step() {
  local name="$1"
  shift
  echo
  echo "==> $name"
  if "$@"; then
    record "$name" PASS
  else
    fail "$name"
  fi
}

step "cargo fmt --check" cargo fmt --all -- --check
step "cargo clippy" cargo clippy --workspace --all-targets -j "$JOBS" -- -D warnings
step "cargo test" cargo test --workspace -j "$JOBS"
step "brief linter" lint_briefs

# Byte identity of one conversion against its baseline.
# identity NAME BASELINE_FILE EXTRA_ARGS...
identity() {
  local name="$1" baseline="$2"
  shift 2
  local out="$SCRATCH/$name" layer expected actual
  echo "-- converting $name"
  "$BIN" all "$@" --fnv "$FNV" --f2 "$F2" --out "$out" || {
    echo "gate: conversion of $name failed" >&2
    return 1
  }
  layer="$out/mods/mojave"
  [ -d "$layer" ] || {
    echo "gate: $layer was not produced" >&2
    return 1
  }
  expected="$(wc -l <"$baseline" | tr -d ' ')"
  actual="$(find "$layer" -type f | wc -l | tr -d ' ')"
  if [ "$expected" != "$actual" ]; then
    echo "gate: $name has $actual files, baseline lists $expected" >&2
    return 1
  fi
  (builtin cd "$layer" && sha256sum -c --quiet "$baseline") || {
    echo "gate: $name differs from its baseline" >&2
    return 1
  }
  echo "-- $name: $actual files identical"
}

real_checks() {
  BIN="${GATE_BIN:-}"
  if [ -z "$BIN" ]; then
    cargo build -p mojave-convert -j "$JOBS" || return 1
    BIN="$ROOT/target/debug/mojave-convert"
  fi
  [ -x "$BIN" ] || {
    echo "gate: no converter binary at $BIN" >&2
    return 1
  }
  case "$SCRATCH" in
    "" | "/" | "$ROOT" | "$HOME") echo "gate: refusing to wipe '$SCRATCH'" >&2; return 1 ;;
  esac
  rm -rf "$SCRATCH"
  mkdir -p "$SCRATCH"
  identity goodsprings "$BASELINES/baseline-goodsprings-2.sha" || return 1
  identity saloon "$BASELINES/baseline-saloon.sha" \
    --standalone --slice "$ROOT/slices/saloon.toml" || return 1
}

skip_reason=""
if [ "$FAST" = 1 ]; then
  skip_reason="--fast"
elif [ -z "$FNV" ] || [ -z "$F2" ]; then
  skip_reason="FNV_DIR / F2_DIR not set"
elif [ ! -d "$FNV" ] || [ ! -d "$F2" ]; then
  skip_reason="install directory not found"
elif [ ! -f "$BASELINES/baseline-goodsprings-2.sha" ] || [ ! -f "$BASELINES/baseline-saloon.sha" ]; then
  skip_reason="baselines not found in $BASELINES"
fi

if [ -n "$skip_reason" ]; then
  echo
  echo "SKIP: byte-identity checks ($skip_reason)"
  if [ "$FAST" != 1 ] && [ "${GATE_REQUIRE_REAL:-0}" = 1 ]; then
    fail "byte identity (required, skipped: $skip_reason)"
  fi
  record "byte identity ($skip_reason)" SKIP
else
  step "byte identity (goodsprings, saloon)" real_checks
fi

summary
echo "GATE PASS"
