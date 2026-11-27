#!/usr/bin/env bash
# PostToolUse hook for Edit and Write: when the edited file is an engine brief,
# run the clean-room brief linter and hand its findings back to the agent.
# Reads the hook's JSON on stdin; exit 2 makes the findings visible to it.
set -u
ROOT="$(builtin cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
input="$(command cat)"
case "$input" in
  *docs/vault-engine-briefs/*) ;;
  *) exit 0 ;;
esac
if out="$("$ROOT/scripts/gate.sh" briefs 2>&1)"; then
  exit 0
fi
printf 'clean-room brief linter failed after this edit:\n%s\n' "$out" >&2
exit 2
