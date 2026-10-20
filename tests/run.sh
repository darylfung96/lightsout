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

# 8 — a block carries a class and a source, and the report exposes both
reset
NOOP_BLOCK=1 bin/job run noop --class retryable >/dev/null 2>&1
row=$(bin/job status --all --json | python3 -c 'import json,sys; print([r for r in json.load(sys.stdin) if r["name"]=="noop"][0]["class"])')
check "block class is recorded" retryable "$row"
row=$(bin/job status --all --json | python3 -c 'import json,sys; print([r for r in json.load(sys.stdin) if r["name"]=="noop"][0]["source"])')
check "block source defaults to unknown" unknown "$row"
raw=$(cat jobs/noop/state/blocked)
check "blocked is a four-field line" 4 "$(printf '%s\n' "$raw" | awk -F'\t' '{print NF}')"
case "$raw" in
  *$'\t'retryable$'\t'unknown$'\t'"simulated gate failure"*)
    echo "ok   blocked line is <iso> CLASS SOURCE REASON" ;;
  *) echo "FAIL blocked line fields: $raw"; fails=$((fails + 1)) ;;
esac
out=$(bin/job run noop 2>&1); rc=$?
check "retryable repeat exits 2" 2 "$rc"
case "$out" in
  *"blocked since"*"[retryable]"*"bin/job retry noop"*)
    echo "ok   retryable block names retry" ;;
  *) echo "FAIL retryable hint missing: $out"; fails=$((fails + 1)) ;;
esac

reset
NOOP_BLOCK=1 bin/job run noop --class judgment --source gate:exists >/dev/null 2>&1
row=$(bin/job status --all --json | python3 -c 'import json,sys; r=[r for r in json.load(sys.stdin) if r["name"]=="noop"][0]; print(r["class"]+" "+r["source"])')
check "class and source are recorded" "judgment gate:exists" "$row"
out=$(bin/job run noop 2>&1); rc=$?
check "judgment repeat exits 2" 2 "$rc"
case "$out" in
  *"[judgment]"*"bin/job retry"*)
    echo "FAIL judgment block names retry"; fails=$((fails + 1)) ;;
  *) echo "ok   judgment block does not name retry" ;;
esac

# 9 — an older two-field block line still reads as human, reason intact
reset
mkdir -p jobs/noop/state
printf '%s\tsimulated gate failure (NOOP_BLOCK=1)\n' "$(date -Iseconds)" > jobs/noop/state/blocked
row=$(bin/job status --all --json | python3 -c 'import json,sys; r=[r for r in json.load(sys.stdin) if r["name"]=="noop"][0]; print(r["class"]+" "+r["source"]+" "+r["detail"])')
check "two-field block reads as human" "human unknown simulated gate failure (NOOP_BLOCK=1)" "$row"
out=$(bin/job run noop 2>&1); rc=$?
check "legacy block still exits 2" 2 "$rc"
case "$out" in
  *"[human]"*"bin/job unblock noop"*)
    echo "ok   human block names unblock" ;;
  *) echo "FAIL human hint missing: $out"; fails=$((fails + 1)) ;;
esac

reset
echo
if [ "$fails" -eq 0 ]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
