#!/usr/bin/env bash
# noop — fixture job. Exercises the guards and gates with no external services.
# Used by tests/run.sh; it is also the smallest complete example of a job.
set -euo pipefail
. "$REPO/lib/guards.sh"
. "$REPO/lib/memory.sh"

stage_write() { mkdir -p "$OUT"; printf 'hello %s\n' "$(date -Iseconds)" > "$OUT/hello.txt"; }

u_guard_day
u_guard_lock
u_guard_attempts
u_guard_disk 100

u_sh "stage 1: write artifact" stage_write
u_gate exists --file "$OUT/hello.txt"

if [ "${NOOP_BLOCK:-0}" = 1 ]; then
  # NOOP_CLASS lets the fixture raise each class; the allowlisted repair for it
  # is touch-missing (lib/repairs.sh), which `bin/job retry noop` runs.
  u_block --class "${NOOP_CLASS:-${U_BLOCK_CLASS:-human}}" \
          "simulated gate failure (NOOP_BLOCK=1)"
fi

u_say "stage 2: memory pass"
u_memory_pass || u_block "memory pass failed"

u_done
u_say "DONE"
