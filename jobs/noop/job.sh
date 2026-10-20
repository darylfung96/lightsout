#!/usr/bin/env bash
# noop — fixture job. Exercises the guards and gates with no external services.
# Used by tests/run.sh; it is also the smallest complete example of a job.
set -euo pipefail
. "$REPO/lib/guards.sh"
. "$REPO/lib/memory.sh"

stage_write() { mkdir -p "$OUT"; printf 'hello %s\n' "$(date -Iseconds)" > "$OUT/hello.txt"; }

u_guard_day
u_guard_lock
# The fixture shares one cross-job resource named fixture. It is taken before
# the attempt is counted, because a busy resource is a hold and a hold is not a
# try: otherwise three busy ticks would read as the attempt cap and block a job
# that never ran. Tests hold the lock from another shell (tests/run.sh, Task 9).
u_resource fixture
u_guard_attempts
u_guard_disk 100

# Stage 1 — the per-stage cap is checked before the work, so a stage that keeps
# failing cannot spend the whole job's day. NOOP_FAIL_STAGE makes it fail, which
# is the fixture's way of proving the cap: the third attempt is a human block.
u_guard_stage 1
if [ "${NOOP_FAIL_STAGE:-0}" = 1 ]; then
  u_block --class human "stage 1 failed (fixture)"
fi

# Stage 1 is the only stage that costs money. Its artifact is the notebook the
# contract makes every paid stage consult: a run that finds hello.txt already
# written has already paid for this unit, so a retry after a partial failure
# re-checks the same gate without charging the 25c a second time. The charge is
# booked with the artifact, never before it, so the ledger never claims money
# for work that did not happen.
if [ -s "$OUT/hello.txt" ]; then
  u_say "stage 1: hello.txt already on disk, paid work skipped"
else
  u_sh "stage 1: write artifact" stage_write
  u_spend 25 "stage 1"
fi
u_gate exists --file "$OUT/hello.txt"

# The cap is checked after the charge because spent money cannot be unspent:
# the run that crosses the ceiling is the run that stops and says so, and the
# block (plus the day and attempt guards) keeps every later tick from spending
# again. U_SPEND_CAP overrides the fixture's 100c daily budget.
u_guard_spend "${U_SPEND_CAP:-100}"

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
