#!/usr/bin/env bash
# video-prune — remove render intermediates for episodes that already shipped.
#
# The policy is narrow on purpose: an episode is eligible only when its master
# exists and pipeline/publish/queue.json says `posted`. Only
# episodes/<NNN>/comp/<name>/renders/ is removed; media, audio, comp assets,
# snapshots, the outbox and the queue are never touched. The last PRUNE_KEEP
# episodes stay fully intact so a recent episode can still be re-rendered.
#
# Dry-run is the default. PRUNE_APPLY=1 is the scheduled form; it deletes and
# marks the day done. A dry-run writes the plan but does not mark done, so the
# apply can still run later the same day.
#
# Contract: docs/run-contract.md. Spec: jobs/video-prune/spec.md.
set -euo pipefail
. "$REPO/lib/guards.sh"
. "$REPO/lib/memory.sh"

VIDEO_REPO="${VIDEO_REPO:-$HOME/programming/video-automation}"
case "$VIDEO_REPO" in
  /*) ;;
  *)  VIDEO_REPO="$PWD/$VIDEO_REPO" ;;
esac
[ -d "$VIDEO_REPO" ] || u_block --class human --source config \
  "VIDEO_REPO is not a directory: $VIDEO_REPO"
[ -f "$VIDEO_REPO/pipeline/publish/queue.json" ] || u_block --class human --source config \
  "no queue.json in $VIDEO_REPO"

PRUNE_APPLY="${PRUNE_APPLY:-0}"
PRUNE_KEEP="${PRUNE_KEEP:-3}"
PRUNE_MIN_AGE_HOURS="${PRUNE_MIN_AGE_HOURS:-24}"
# `plan` is read-only: no log file, no manifest, and never an apply even if the
# environment says PRUNE_APPLY=1.
if [ "$DRY" = 1 ]; then
  LOG=/dev/null
  PRUNE_APPLY=0
  plan=/dev/null
else
  plan="$STATE/prune-$UNIT.tsv"
fi
applied="$STATE/applied-$UNIT.tsv"
case "$PRUNE_APPLY" in
  0|1) ;;
  *) u_block --class human --source config "PRUNE_APPLY must be 0 or 1" ;;
esac
case "$PRUNE_KEEP" in
  ''|*[!0-9]*) u_block --class human --source config "PRUNE_KEEP must be a whole number" ;;
esac
case "$PRUNE_MIN_AGE_HOURS" in
  ''|*[!0-9.]*) u_block --class human --source config \
    "PRUNE_MIN_AGE_HOURS must be a number" ;;
esac

# No notes are written, so the memory pass is deliberately skipped: there is
# nothing for it to save, and a session-bound receipt cannot be produced by a
# launchd job. This is not a silent bypass; it is the no-note case.
U_MEMORY=0
export U_MEMORY

u_guard_day
u_guard_lock
# A build owns the episode tree while it is rendering. Never prune under it.
if [ -d "$VIDEO_REPO/logs/daily-build.lock" ]; then
  u_held "the video driver's build lock exists ($VIDEO_REPO/logs/daily-build.lock)"
fi
u_guard_attempts

set +e
out=$(python3 "$JOB_DIR/prune.py" \
  --video-repo "$VIDEO_REPO" \
  --keep "$PRUNE_KEEP" \
  --min-age-hours "$PRUNE_MIN_AGE_HOURS" \
  --apply "$PRUNE_APPLY" \
  --plan "$plan" \
  --applied "$applied" 2>&1)
rc=$?
set -e
printf '%s\n' "$out" | while IFS= read -r line; do
  [ -n "$line" ] && u_say "$line"
done
if [ "$rc" -ne 0 ]; then
  u_block --class human --source prune "pruner failed (rc $rc); see $plan"
fi

u_gate exists --file "$plan"
if [ "$PRUNE_APPLY" = 1 ]; then
  u_gate exists --file "$applied"
  u_memory_pass || u_block "memory pass failed"
  u_done
  u_say "DONE: render intermediates pruned; plan $plan"
else
  u_memory_pass || u_block "memory pass failed"
  u_say "dry-run: no files deleted; set PRUNE_APPLY=1 to apply"
fi
