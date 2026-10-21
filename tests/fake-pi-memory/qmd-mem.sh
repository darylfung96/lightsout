#!/usr/bin/env bash
# fake qmd-mem.sh — the test double for ~/agent-memory/qmd-mem.sh. Tests point
# U_PI_MEMORY here so no check ever reaches the real store: this script records
# the call and exits with FAKE_MEMORY_RC, or fails the save step when
# FAKE_MEMORY_FAIL=save (tests/run.sh, Task 11).
set -euo pipefail

if [ -n "${FAKE_MEMORY_LOG:-}" ]; then
  printf 'qmd-mem %s\n' "$*" >> "$FAKE_MEMORY_LOG"
fi
if [ "${FAKE_MEMORY_FAIL:-}" = save ]; then
  echo "fake qmd-mem: save failed" >&2
  exit 1
fi
exit "${FAKE_MEMORY_RC:-0}"
