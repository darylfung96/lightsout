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

# spend_today — what the dashboard says the fixture has spent today, in cents.
spend_today() {
  bin/job status --all --json | python3 -c '
import json,sys
print([r for r in json.load(sys.stdin) if r["name"]=="noop"][0]["spend_today"])'
}

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

# 10 — diagnose explains a block without changing a byte of it
reset
NOOP_BLOCK=1 bin/job run noop >/dev/null 2>&1
printf '[%s] FAIL exists: simulated gate failure\n' "$(date '+%F %T')" >> "jobs/noop/logs/$(date +%F).log"
before=$(md5 -q jobs/noop/state/blocked)
bin/job diagnose noop >/dev/null; check "diagnose exits 0" 0 "$?"
after=$(md5 -q jobs/noop/state/blocked)
check "diagnose leaves the block untouched" "$before" "$after"
[ -s jobs/noop/state/diagnosis.md ]; check "diagnosis written" 0 "$?"
for section in "Cause candidates" "Evidence" "Next commands"; do
  grep -q "^## $section" jobs/noop/state/diagnosis.md
  check "diagnosis has a $section section" 0 "$?"
done
grep -q "FAIL exists: simulated gate failure" jobs/noop/state/diagnosis.md
check "diagnosis quotes the last failing gate line" 0 "$?"
grep -q "stage 1: write artifact" jobs/noop/state/diagnosis.md
check "diagnosis carries the log tail" 0 "$?"
grep -q "bin/job unblock noop" jobs/noop/state/diagnosis.md
check "human block names unblock" 0 "$?"
out=$(bin/job diagnose noop 2>&1); rc=$?
check "diagnose prints and exits 0" 0 "$rc"
case "$out" in *"# Diagnosis"*) echo "ok   diagnosis printed on stdout";; *) echo "FAIL diagnosis not printed"; fails=$((fails + 1));; esac
bin/job run noop >/dev/null 2>&1; check "diagnose does not clear the block" 2 "$?"
reset
bin/job diagnose noop >/dev/null 2>&1; check "diagnose works when not blocked" 0 "$?"
grep -qi "not blocked" jobs/noop/state/diagnosis.md; check "unblocked diagnosis says so" 0 "$?"

# 11 — retry is an allowlist: a retryable block, a registered repair, no verdict
reset
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job run noop >/dev/null 2>&1
bin/job retry noop >/dev/null 2>&1; check "retry runs after a retryable block" 0 "$?"
[ -f "jobs/noop/state/done/$(date +%F)" ]; check "retry re-runs the job to done" 0 "$?"
[ -f jobs/noop/state/blocked ]; check "retry cleared the block on success" 1 "$?"
check "one repair counted" 1 "$(cat "jobs/noop/state/repairs/$(date +%F).count" 2>/dev/null)"
grep -q "retry: repair touch-missing" "jobs/noop/logs/$(date +%F).log"
check "the repair is logged by name" 0 "$?"
case "$(tail -1 jobs/noop/state/last)" in
  *$'\t'0$'\t'retry) echo "ok   retry is recorded as its own verb" ;;
  *) echo "FAIL retry not recorded: $(tail -1 jobs/noop/state/last)"; fails=$((fails + 1)) ;;
esac

reset
NOOP_BLOCK=1 NOOP_CLASS=human bin/job run noop >/dev/null 2>&1
bin/job retry noop >/dev/null 2>&1; check "retry refuses a human block" 2 "$?"
[ -f jobs/noop/state/blocked ]; check "refused retry keeps the block" 0 "$?"
[ -d jobs/noop/state/repairs ]; check "refused retry runs no repair" 1 "$?"
out=$(bin/job retry noop 2>&1)
case "$out" in
  *"diagnosis.md"*) echo "ok   refusal points at the diagnosis" ;;
  *) echo "FAIL refusal does not point at the diagnosis: $out"; fails=$((fails + 1)) ;;
esac

reset
NOOP_BLOCK=1 NOOP_CLASS=judgment bin/job run noop >/dev/null 2>&1
bin/job retry noop >/dev/null 2>&1; check "retry refuses a judgment block" 2 "$?"

reset
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job run noop >/dev/null 2>&1
before=$(find jobs/noop/state -type f | wc -l | tr -d ' ')
out=$(bin/job retry noop --repair does-not-exist 2>&1); rc=$?
check "retry refuses an unknown repair" 2 "$rc"
after=$(find jobs/noop/state -type f | wc -l | tr -d ' ')
check "unknown repair touches no state" "$before" "$after"
case "$out" in
  *"touch-missing"*) echo "ok   unknown repair lists the allowlist" ;;
  *) echo "FAIL unknown repair does not list the allowlist: $out"; fails=$((fails + 1)) ;;
esac

reset
bin/job retry noop >/dev/null 2>&1; check "retry with nothing blocked is a held no-op" 0 "$?"
[ -d jobs/noop/state/repairs ]; check "no repair ran without a block" 1 "$?"

reset
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job run noop >/dev/null 2>&1
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job retry noop >/dev/null 2>&1
check "a repair cannot launder a bad artifact" 2 "$?"
[ -f jobs/noop/state/blocked ]; check "a failing retry is blocked again" 0 "$?"

reset
NOOP_BLOCK=1 bin/job run noop --class retryable >/dev/null 2>&1
bin/job diagnose noop >/dev/null 2>&1; check "diagnose on a retryable block exits 0" 0 "$?"
grep -q "touch-missing" jobs/noop/state/diagnosis.md; check "diagnose lists the registered repair" 0 "$?"

reset
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job run noop >/dev/null 2>&1
# Both caps are lifted so this loop tests MAX_REPAIRS and nothing else: the
# third run would otherwise be the stage cap (Task 4), not the repair cap.
NOOP_BLOCK=1 NOOP_CLASS=retryable MAX_ATTEMPTS=99 MAX_STAGE_ATTEMPTS=99 MAX_REPAIRS=2 bin/job retry noop >/dev/null 2>&1
check "first retry under the cap runs" 2 "$?"
NOOP_BLOCK=1 NOOP_CLASS=retryable MAX_ATTEMPTS=99 MAX_STAGE_ATTEMPTS=99 MAX_REPAIRS=2 bin/job retry noop >/dev/null 2>&1
check "second retry under the cap runs" 2 "$?"
out=$(NOOP_BLOCK=1 NOOP_CLASS=retryable MAX_ATTEMPTS=99 MAX_STAGE_ATTEMPTS=99 MAX_REPAIRS=2 bin/job retry noop 2>&1); rc=$?
check "repair cap blocks the third retry" 2 "$rc"
case "$out" in
  *"repair cap"*) echo "ok   repair cap is reported" ;;
  *) echo "FAIL repair cap not reported: $out"; fails=$((fails + 1)) ;;
esac
check "two repairs recorded today" 2 "$(cat "jobs/noop/state/repairs/$(date +%F).count" 2>/dev/null)"

# 12 — per-stage attempt caps: one stage cannot burn the whole job's budget
reset
for i in 1 2 3; do NOOP_FAIL_STAGE=1 bin/job run noop >/dev/null 2>&1; done
bin/job status --all --json | python3 -c '
import json,sys
r=[x for x in json.load(sys.stdin) if x["name"]=="noop"][0]
assert r["state"]=="blocked" and "stage" in r["detail"], r'
check "a failing stage blocks with a stage detail" 0 "$?"

reset
for i in 1 2 3; do
  NOOP_FAIL_STAGE=1 MAX_ATTEMPTS=99 bin/job run noop >/dev/null 2>&1
  [ "$i" = 3 ] || bin/job unblock noop >/dev/null 2>&1
done
row=$(bin/job status --all --json | python3 -c '
import json,sys
r=[x for x in json.load(sys.stdin) if x["name"]=="noop"][0]
print("%s|%s" % (r["class"], r["detail"]))')
check "third stage attempt is a human block" \
  "human|stage 1 attempt cap reached (2 of 2 today)" "$row"
check "only the stage's own attempts are counted" 2 \
  "$(find jobs/noop/state/attempts -maxdepth 1 -name "$(date +%F).1.attempt-*" | wc -l | tr -d ' ')"

# 13 — spend is recorded per unit, capped, and never charged twice
reset
bin/job run noop >/dev/null 2>&1
check "the paid run completes" 0 "$?"
check "spend_today sums today's ledger" 25 "$(spend_today)"
check "the ledger books 25c for stage 1" "25|stage 1" \
  "$(awk -F'\t' 'NR==1{print $2"|"$3}' jobs/noop/state/ledger.tsv)"
check "the ledger line names the unit" "$(date +%F)" \
  "$(awk -F'\t' 'NR==1{print $4}' jobs/noop/state/ledger.tsv)"
# the day guard holds a second run before it can reach the paid stage
bin/job run noop >/dev/null 2>&1
check "a held re-run books nothing" 25 "$(spend_today)"

# only today's lines are today's money: an old line is neither reported nor capped
reset
mkdir -p jobs/noop/state
printf '2020-01-01T00:00:00-0600\t900\tstage 1\t2020-01-01\n' > jobs/noop/state/ledger.tsv
U_SPEND_CAP=100 bin/job run noop >/dev/null 2>&1
check "a run under the cap is not blocked by old spend" 0 "$?"
check "yesterday's spend is not counted today" 25 "$(spend_today)"

# the run that crosses the cap stops and names the number
reset
U_SPEND_CAP=10 bin/job run noop >/dev/null 2>&1; check "spend cap blocks" 2 "$?"
check "the spend block is a human stop" \
  "human|spend cap reached (25 of 10 cents today)" \
  "$(bin/job status --all --json | python3 -c '
import json,sys
r=[x for x in json.load(sys.stdin) if x["name"]=="noop"][0]
print("%s|%s" % (r["class"], r["detail"]))')"

# review focus 3: a partial failure that is retried pays for the unit once
reset
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job run noop >/dev/null 2>&1
bin/job retry noop >/dev/null 2>&1; check "the retried run finishes" 0 "$?"
check "a retry does not double-charge" 25 "$(spend_today)"

reset
echo
if [ "$fails" -eq 0 ]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
