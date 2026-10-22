#!/usr/bin/env bash
# brag-sweep — the daily job sweep for the Brag resume repo.
#
# The engine is jobtool/sweep.sh in that repo: doctor, digest, a headless agent
# pass that annotates the new postings, then render and a dashboard screenshot.
# LightsOut owns the clock, the day guard, the attempt cap, the lock and the
# gates; the engine owns nothing but the sweep, so it stays runnable by hand.
#
# The repo moved out of ~/Documents on 2026-10-21 because launchd may not read
# that folder without Full Disk Access. The engine stays in the repo; only the
# clock, guards and gates live here in the office.
set -euo pipefail
. "${LIGHTSOUT_LIB:-$REPO/lib}/guards.sh"

BRAG="${BRAG_REPO:-$HOME/programming/Brag}"
ENGINE="$BRAG/jobtool/sweep.sh"
DIGEST="$BRAG/jobtool/output/digests/$(date +%F).md"
PAGE="$BRAG/jobtool/output/index.html"

[ -d "$BRAG" ] || u_block --class human --source preflight "BRAG_REPO is not a directory: $BRAG"
[ -x "$ENGINE" ] || u_block --class human --source preflight "no engine at $ENGINE"

if [ "$DRY" = 1 ]; then
  u_say "plan: sweep $BRAG"
  u_say "plan: doctor, then digest -q (new postings only)"
  u_say "plan: annotate the new postings with a ${U_RUNNER:-dsh} headless agent (timeout ${U_AGENT_TIMEOUT:-900}s)"
  u_say "plan: render --top 25, then screenshot jobtool/output/dashboard.png"
  u_say "plan: gates exist $DIGEST and exist $PAGE"
  exit 0
fi

u_guard_day
u_guard_lock
u_guard_attempts
u_guard_disk

u_say "sweep: $BRAG"
if ! "$ENGINE"; then
  u_block --class retryable --source sweep "the sweep engine failed"
fi

# The engine exits 0 on "nothing new today" too, so the gates ask only that the
# day has a digest and a page behind it, not that the digest reports a find.
u_gate exists --file "$DIGEST"
u_gate exists --file "$PAGE"

u_done
