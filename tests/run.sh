#!/usr/bin/env bash
# tests/run.sh — exercise the contract against the noop fixture. No network.
set -uo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO"
export U_NOTIFY=0 U_MEMORY=0
fails=0

check() { # check <label> <want> <got>
  if [ "$2" = "$3" ]; then printf 'ok   %s\n' "$1"
  else printf 'FAIL %s (want %s, got %s)\n' "$1" "$2" "$3"; fails=$((fails + 1)); fi
}

reset() { rm -rf jobs/noop/state jobs/noop/out jobs/noop/logs; }

# 1 — a fresh run finishes and writes its marker
reset
bin/job run noop >/dev/null 2>&1; check "fresh run exits 0" 0 "$?"
[ -f "jobs/noop/state/done/$(date +%F)" ]; check "done marker written" 0 "$?"
[ -f jobs/noop/out/hello.txt ]; check "artifact written" 0 "$?"

# 2 — the same day holds instead of rebuilding
out=$(bin/job run noop 2>&1); rc=$?
check "same-day run exits 0" 0 "$rc"
case "$out" in *"held:"*) echo "ok   same-day run held";; *) echo "FAIL same-day run not held"; fails=$((fails + 1));; esac

# 3 — a failed gate blocks, and notifies exactly once
reset
NOOP_BLOCK=1 bin/job run noop >/dev/null 2>&1; check "gate failure exits 2" 2 "$?"
[ -f jobs/noop/state/blocked ]; check "blocked marker written" 0 "$?"
alerts_before=$(grep -c "simulated" logs/alerts.log 2>/dev/null || true)
out=$(bin/job run noop 2>&1); rc=$?
check "blocked repeat exits 2" 2 "$rc"
case "$out" in *"blocked since"*) echo "ok   repeat reports the block";; *) echo "FAIL repeat silent"; fails=$((fails + 1));; esac
alerts_after=$(grep -c "simulated" logs/alerts.log 2>/dev/null || true)
check "no second notification" "$alerts_before" "$alerts_after"

# 4 — a human clears it and the job runs again
bin/job unblock noop >/dev/null; check "unblock exits 0" 0 "$?"
[ -f jobs/noop/state/blocked ]; check "blocked marker cleared" 1 "$?"
reset
bin/job run noop >/dev/null 2>&1; check "run after unblock exits 0" 0 "$?"

# 5 — plan touches nothing
reset
before=$(find jobs/noop/state jobs/noop/out -type f 2>/dev/null | wc -l | tr -d ' ')
bin/job plan noop >/dev/null 2>&1; check "plan exits 0" 0 "$?"
after=$(find jobs/noop/state jobs/noop/out -type f 2>/dev/null | wc -l | tr -d ' ')
check "plan wrote no files" "$before" "$after"

# 6 — status is machine-readable and the page renders
bin/job status --all --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert isinstance(d,list) and d' \
  && echo "ok   status --json parses" || { echo "FAIL status --json"; fails=$((fails + 1)); }
bin/report --html /tmp/lightsout-status.html >/dev/null; check "report renders html" 0 "$?"

# 7 — the exception case is visible to the dashboard
reset
NOOP_BLOCK=1 bin/job run noop >/dev/null 2>&1
bin/job status --all --json | python3 -c '
import json,sys
row=[r for r in json.load(sys.stdin) if r["name"]=="noop"][0]
assert row["state"]=="blocked", row
assert "simulated" in row["detail"], row
' && echo "ok   blocked job surfaces on the dashboard" || { echo "FAIL dashboard state"; fails=$((fails + 1)); }

reset
echo
if [ "$fails" -eq 0 ]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
