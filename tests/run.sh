#!/usr/bin/env bash
# tests/run.sh — exercise the contract against the noop fixture. No network.
set -uo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO"
export U_NOTIFY=0 U_MEMORY=0
fails=0

# The manager is the Rust binary. Build it here so a fresh checkout can run the
# shell reference suite without a separate setup step.
(cd "$REPO" && cargo build --release) >/dev/null || {
  echo "tests/run.sh: cargo build --release failed" >&2
  exit 1
}
cp "$REPO/target/release/lightsout" "$REPO/bin/lightsout"

# The suite drives fixed paths under jobs/, so two concurrent runs trample each
# other and report phantom failures. Hold a lock instead.
tests_lock="${TMPDIR:-/tmp}/lightsout-tests.lock"
if ! mkdir "$tests_lock" 2>/dev/null; then
  echo "another tests/run.sh holds $tests_lock — refusing to run"
  exit 75
fi
trap 'rmdir "$tests_lock" 2>/dev/null || true' EXIT

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

# stale_of_noop — what the dashboard says about the fixture's missed run.
stale_of_noop() {
  bin/job status --all --json | python3 -c '
import json,sys
print([r for r in json.load(sys.stdin) if r["name"]=="noop"][0]["stale"])'
}

# stale_alerts — stale alerts raised for the fixture so far. Counted from the
# log the notifier appends to, so repeated test runs do not confuse the count.
stale_alerts() { awk '/noop stale/' logs/alerts.log 2>/dev/null | wc -l | tr -d ' '; }

# blocked_alerts — block notifications for the fixture recorded so far. The
# alert file outlives one test run, so a check compares before and after.
blocked_alerts() {
  [ -f logs/alerts.log ] || { echo 0; return; }
  awk '/noop blocked/ { n++ } END { print n + 0 }' logs/alerts.log
}

# agent_invocations — how many times the fake runner ran in Task 12's checks.
# The log does not exist until the first call, and a missing log is zero calls.
agent_invocations() {
  if [ -f "${agent_calls:-}" ]; then wc -l < "$agent_calls" | tr -d ' '
  else echo 0; fi
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

# 14 — the run that never happened: stale is read from files, and a legitimate
# hold is not a missed run (review focus 4)
reset
check "a job with no expectation is never stale" False "$(stale_of_noop)"

reset
mkdir -p jobs/noop/state
printf '00:00\n' > jobs/noop/state/expect
check "a job past its time with nothing done is stale" True "$(stale_of_noop)"
bin/job run noop >/dev/null 2>&1
check "a job done today is not stale" False "$(stale_of_noop)"

# the day guard's hold is a legitimate reason not to have run again
out=$(bin/job run noop 2>&1); rc=$?
check "the second run is held" 0 "$rc"
case "$out" in
  *"held:"*) echo "ok   the day guard hold is reported" ;;
  *) echo "FAIL day guard hold not reported: $out"; fails=$((fails + 1)) ;;
esac
check "a day-guard hold is not stale" False "$(stale_of_noop)"

# the lock's hold: not stale while another run holds it, and a run that ended
# held suppresses the alarm for the rest of the day
reset
mkdir -p jobs/noop/state/lock
printf '00:00\n' > jobs/noop/state/expect
printf '%s\n' "$$" > jobs/noop/state/lock/pid
check "a held lock is not stale" False "$(stale_of_noop)"
out=$(bin/job run noop 2>&1); rc=$?
check "a run under another run's lock exits 0" 0 "$rc"
case "$out" in
  *"held: another run"*) echo "ok   the lock hold is reported" ;;
  *) echo "FAIL lock hold not reported: $out"; fails=$((fails + 1)) ;;
esac
rm -rf jobs/noop/state/lock
check "a run that ended held is not a missed run" False "$(stale_of_noop)"

# 15 — job check: one alarm per job per day, and a missed block stays non-zero
reset
mkdir -p jobs/noop/state
printf '00:00\n' > jobs/noop/state/expect
before=$(stale_alerts)
bin/job check >/dev/null 2>&1; check "check exits 0 for a stale job" 0 "$?"
[ -f "jobs/noop/state/alerts/$(date +%F).stale" ]
check "check writes one stale marker" 0 "$?"
check "check notifies once" 1 "$(( $(stale_alerts) - before ))"
bin/job check >/dev/null 2>&1
check "a second check does not notify again" 1 "$(( $(stale_alerts) - before ))"
[ -f "jobs/noop/state/done/$(date +%F)" ]; check "check never runs the job" 1 "$?"
[ -f jobs/noop/out/hello.txt ]; check "check writes no artifact" 1 "$?"

reset
mkdir -p jobs/noop/state
printf '00:00\n' > jobs/noop/state/expect
bin/job run noop >/dev/null 2>&1
before=$(stale_alerts)
bin/job check >/dev/null 2>&1; check "check exits 0 when the job is done" 0 "$?"
[ -f "jobs/noop/state/alerts/$(date +%F).stale" ]
check "a done job raises no stale marker" 1 "$?"
check "a done job raises no stale alert" "$before" "$(stale_alerts)"

reset
mkdir -p jobs/noop/state
printf '00:00\n' > jobs/noop/state/expect
NOOP_BLOCK=1 bin/job run noop >/dev/null 2>&1
before=$(stale_alerts)
bin/job check >/dev/null 2>&1; check "a missed run that is blocked exits 2" 2 "$?"
check "a blocked job does not raise the stale alert" "$before" "$(stale_alerts)"
[ -f "jobs/noop/state/alerts/$(date +%F).stale" ]
check "a blocked job writes no stale marker" 1 "$?"

# an earlier hold today must not mask a later block: the block needs a human
reset
mkdir -p jobs/noop/state/lock
printf '00:00\n' > jobs/noop/state/expect
printf '%s\n' "$$" > jobs/noop/state/lock/pid
bin/job run noop >/dev/null 2>&1
rm -rf jobs/noop/state/lock
NOOP_BLOCK=1 bin/job run noop >/dev/null 2>&1
bin/job check >/dev/null 2>&1; check "a hold does not mask a later block" 2 "$?"

# 16 — notifications: each configured channel is tried, the alert file always
# records the event, and no channel can fail the job (Task 7)
notify_stubs() { # notify_stubs — a PATH whose osascript and curl record their args
  local d
  d=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-notify.XXXXXX") || return 1
  for c in osascript curl; do
    cat > "$d/$c" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >> "${NOTIFY_LOG:-/dev/null}"
STUB
    chmod +x "$d/$c"
  done
  printf '%s\n' "$d"
}

reset
stub=$(notify_stubs)
NOTIFY_LOG=/tmp/lightsout-notify
export NOTIFY_LOG
rm -f "$NOTIFY_LOG"
before=$(blocked_alerts)
NOOP_BLOCK=1 U_NOTIFY=1 U_NOTIFY_CMD='printf "%s|%s\n" >> /tmp/lightsout-notify' \
  U_NOTIFY_WEBHOOK='http://example.invalid/hook' PATH="$stub:$PATH" \
  bin/job run noop >/dev/null 2>&1
check "channels do not change the verdict" 2 "$?"
grep -q "LightsOut: noop blocked|simulated gate failure" "$NOTIFY_LOG"
check "the command channel gets the title and the body" 0 "$?"
grep -q "display notification" "$NOTIFY_LOG"
check "the desktop channel falls back through osascript" 0 "$?"
grep -q "http://example.invalid/hook" "$NOTIFY_LOG"
check "the webhook channel fires" 0 "$?"
grep -q '"body": "simulated gate failure' "$NOTIFY_LOG"
check "the webhook carries the block as JSON" 0 "$?"
check "the alert is recorded once" 1 "$(( $(blocked_alerts) - before ))"

reset
rm -f "$NOTIFY_LOG"
before=$(blocked_alerts)
NOOP_BLOCK=1 U_NOTIFY=0 U_NOTIFY_CMD='printf "%s|%s\n" >> /tmp/lightsout-notify' \
  U_NOTIFY_WEBHOOK='http://example.invalid/hook' PATH="$stub:$PATH" \
  bin/job run noop >/dev/null 2>&1
check "a silenced notification still blocks" 2 "$?"
[ -f "$NOTIFY_LOG" ]; check "U_NOTIFY=0 silences every channel" 1 "$?"
check "a silenced notification is still recorded" 1 "$(( $(blocked_alerts) - before ))"

reset
NOOP_BLOCK=1 U_NOTIFY=1 U_NOTIFY_CMD=false PATH="$stub:$PATH" \
  bin/job run noop >/dev/null 2>&1
check "a failing channel does not fail the job" 2 "$?"
[ -f jobs/noop/state/blocked ]; check "a failing channel keeps the block" 0 "$?"

reset
t0=$(date +%s)
NOOP_BLOCK=1 U_NOTIFY=1 U_NOTIFY_CMD='sleep 30; :' PATH="$stub:$PATH" \
  bin/job run noop >/dev/null 2>&1
check "a hanging channel does not change the verdict" 2 "$?"
elapsed=$(( $(date +%s) - t0 ))
[ "$elapsed" -ge 4 ] && [ "$elapsed" -lt 15 ]
check "a hanging channel is cut off at 5s, not at its own pace (${elapsed}s)" 0 "$?"

rm -f "$NOTIFY_LOG"
rm -rf "$stub"
unset NOTIFY_LOG

# 17 — run history: every completed run is one line with its own duration, the
# page shows the last five, and the JSON window is the last ten (Task 8)
reset
bin/job status --all --json | python3 -c '
import json, sys
r = [x for x in json.load(sys.stdin) if x["name"] == "noop"][0]
assert r["runs"] == [] and r["last_duration"] is None, r
' && echo "ok   a job with no runs has an empty history" \
  || { echo "FAIL empty history"; fails=$((fails + 1)); }

reset
bin/job run noop >/dev/null 2>&1
bin/job run noop >/dev/null 2>&1
n=$(wc -l < jobs/noop/state/runs.tsv 2>/dev/null | tr -d ' ')
check "two runs recorded" 2 "$n"
check "a run line has six fields" 6 \
  "$(awk -F'\t' 'NR==1{print NF}' jobs/noop/state/runs.tsv 2>/dev/null)"
check "the held run is in the history too" 0 \
  "$(awk -F'\t' 'NR==2{print $3}' jobs/noop/state/runs.tsv 2>/dev/null)"
check "a run records its verb and unit" "run|$(date +%F)" \
  "$(awk -F'\t' 'NR==1{print $4"|"$5}' jobs/noop/state/runs.tsv 2>/dev/null)"
bin/job status --all --json | python3 -c '
import json, sys
r = [x for x in json.load(sys.stdin) if x["name"] == "noop"][0]
assert len(r["runs"]) == 2, r["runs"]
first, last = r["runs"]
assert first["verb"] == "run" and first["rc"] == 0, first
assert first["start"].startswith("20") and first["end"].startswith("20"), first
assert isinstance(first["seconds"], int) and first["seconds"] >= 0, first
assert last["seconds"] == r["last_duration"], (last, r["last_duration"])
' && echo "ok   status --json parses runs with durations" \
  || { echo "FAIL status --json runs"; fails=$((fails + 1)); }

# a retry is its own run, and the run that blocked stays in the history
reset
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job run noop >/dev/null 2>&1
bin/job retry noop >/dev/null 2>&1
check "a blocked run keeps its exit in the history" 2 \
  "$(awk -F'\t' 'NR==1{print $3}' jobs/noop/state/runs.tsv 2>/dev/null)"
check "a retry is recorded as its own verb" retry \
  "$(awk -F'\t' 'END{print $4}' jobs/noop/state/runs.tsv 2>/dev/null)"

# the JSON window is the last ten; the page keeps the duration and the last five
reset
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do bin/job run noop >/dev/null 2>&1; done
check "runs.tsv keeps every completed run" 12 \
  "$(wc -l < jobs/noop/state/runs.tsv 2>/dev/null | tr -d ' ')"
bin/job status --all --json | python3 -c '
import json, sys
r = [x for x in json.load(sys.stdin) if x["name"] == "noop"][0]
rows = [l.split("\t") for l in open("jobs/noop/state/runs.tsv").read().splitlines()]
assert len(r["runs"]) == 10, len(r["runs"])
for row, run in zip(rows[-10:], r["runs"]):
    assert (run["start"], run["end"], run["rc"], run["verb"], run["unit"], run["seconds"]) \
        == (row[0], row[1], int(row[2]), row[3], row[4], int(row[5])), (row, run)
assert r["last_duration"] == int(rows[-1][5]), (r["last_duration"], rows[-1])
' && echo "ok   the last ten runs are parsed oldest first" \
  || { echo "FAIL run history window"; fails=$((fails + 1)); }
bin/report --html /tmp/lightsout-history.html >/dev/null
grep -q '<th>duration</th>' /tmp/lightsout-history.html
check "the page names the duration" 0 "$?"
grep -q '<th>last 5 runs</th>' /tmp/lightsout-history.html
check "the page names the last 5 runs" 0 "$?"
check "the page renders five recent runs" 5 \
  "$(grep -o "class='run'" /tmp/lightsout-history.html | wc -l | tr -d ' ')"

# 18 — resource locks: a named resource lets one run through at a time; a busy
# resource is a hold (exit 0, no work), and a lock left by a dead or expired
# holder is reclaimed instead of deadlocking the next run (Task 9)
reset
mkdir -p jobs/noop/state
printf '00:00\n' > jobs/noop/state/expect
# hold the resource from a background shell, then run: the job must hold, not crash
( mkdir -p jobs/noop/state/resources; printf '%s\t%s\n' $$ $(( $(date +%s) + 60 )) > jobs/noop/state/resources/fixture.lock; sleep 3 ) &
sleep 1
lock_before=$(cat jobs/noop/state/resources/fixture.lock)
out=$(bin/job run noop 2>&1); rc=$?
check "busy resource holds" 0 "$rc"
[ -f "jobs/noop/state/done/$(date +%F)" ]; check "a resource hold does no work" 1 "$?"
check "a held run leaves the other run's lock alone" "$lock_before" \
  "$(cat jobs/noop/state/resources/fixture.lock 2>/dev/null)"
case "$out" in
  *"held: resource fixture"*) echo "ok   the resource hold is reported" ;;
  *) echo "FAIL resource hold not reported: $out"; fails=$((fails + 1)) ;;
esac
check "a resource hold is not a missed run" False "$(stale_of_noop)"
wait

reset
( mkdir -p jobs/noop/state/resources; printf '999999\t%s\n' $(( $(date +%s) - 60 )) > jobs/noop/state/resources/fixture.lock )
out=$(bin/job run noop 2>&1); rc=$?
check "expired resource reclaimed" 0 "$rc"
[ -f "jobs/noop/state/done/$(date +%F)" ]; check "a reclaimed resource runs the job" 0 "$?"
[ -f jobs/noop/state/resources/fixture.lock ]; check "a released resource leaves no lock" 1 "$?"
case "$out" in
  *"reclaimed"*) echo "ok   the stale lock is named" ;;
  *) echo "FAIL stale lock not reported: $out"; fails=$((fails + 1)) ;;
esac

# review focus 5: the lock a SIGKILLed holder leaves names a dead pid and an
# unexpired deadline, so the pid is what must reclaim it
reset
mkdir -p jobs/noop/state/resources
bash -c 'printf "%s\t%s\n" "$$" "$(( $(date +%s) + 3600 ))" > "$1"; exec sleep 30' _ \
  jobs/noop/state/resources/fixture.lock &
holder=$!
sleep 1
kill -9 "$holder" 2>/dev/null || true
wait "$holder" 2>/dev/null || true
out=$(bin/job run noop 2>&1); rc=$?
check "a SIGKILLed holder is reclaimed" 0 "$rc"
[ -f "jobs/noop/state/done/$(date +%F)" ]; check "a lock left by SIGKILL does not deadlock" 0 "$?"
[ -f jobs/noop/state/resources/fixture.lock ]; check "the dead holder's lock is removed" 1 "$?"

# U_RESOURCE_WAIT waits for a live holder, then takes the resource and runs
reset
( mkdir -p jobs/noop/state/resources; printf '%s\t%s\n' $$ $(( $(date +%s) + 60 )) > jobs/noop/state/resources/fixture.lock; sleep 2; rm -f jobs/noop/state/resources/fixture.lock ) &
sleep 1
out=$(U_RESOURCE_WAIT=10 bin/job run noop 2>&1); rc=$?
check "a waiting run takes the resource when it frees" 0 "$rc"
[ -f "jobs/noop/state/done/$(date +%F)" ]; check "a waited-for resource runs the job" 0 "$?"
case "$out" in
  *"resource fixture acquired"*) echo "ok   the waiting run logs the acquisition" ;;
  *) echo "FAIL acquisition not logged: $out"; fails=$((fails + 1)) ;;
esac
wait

reset
bin/job plan noop >/dev/null 2>&1
[ -d jobs/noop/state/resources ]; check "plan takes no resource" 1 "$?"

# 19 — the asset approval gate: a shot list passes only when every shot is
# approved and every file is on disk, and the message names the first offender
# (Task 10)
reset
bin/job run noop >/dev/null 2>&1
hello="$PWD/jobs/noop/out/hello.txt"
shots=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-shots.XXXXXX")
: > "$shots/empty.mp4"

# the shot an approval step has not reached yet
printf '[{"shot":1,"query":"staff badge","file":"%s","approved":false}]' "$hello" > /tmp/shots.json
out=$(bin/lightsout gate manifest --file /tmp/shots.json --min 1); rc=$?
check "an unapproved shot fails" 1 "$rc"
case "$out" in
  *"shot 1"*"staff badge"*"not approved"*)
    echo "ok   the failure names the unapproved shot and its query" ;;
  *) echo "FAIL unapproved shot message: $out"; fails=$((fails + 1)) ;;
esac

# the same shot, approved, with the file the manifest points at
printf '[{"shot":1,"query":"staff badge","file":"%s","approved":true}]' "$hello" > /tmp/shots.json
bin/lightsout gate manifest --file /tmp/shots.json --min 1 >/dev/null
check "an approved shot with its file passes" 0 "$?"

# every shot approved but one file missing: approval is not enough
printf '[{"shot":2,"query":"staff badge","file":"%s/nope.mp4","approved":true}]' "$shots" > /tmp/shots.json
out=$(bin/lightsout gate manifest --file /tmp/shots.json --min 1); rc=$?
check "an approved shot with no file fails" 1 "$rc"
case "$out" in
  *"shot 2"*"missing"*) echo "ok   the failure names the missing file" ;;
  *) echo "FAIL missing file message: $out"; fails=$((fails + 1)) ;;
esac

printf '[{"shot":3,"query":"staff badge","file":"%s/empty.mp4","approved":true}]' "$shots" > /tmp/shots.json
bin/lightsout gate manifest --file /tmp/shots.json --min 1 >/dev/null
check "an empty shot file fails" 1 "$?"

# the first unapproved shot is the one named, not merely the first entry
printf '[{"shot":1,"query":"staff badge","file":"%s","approved":true},{"shot":2,"query":"lobby door","file":"%s","approved":false}]' \
  "$hello" "$hello" > /tmp/shots.json
out=$(bin/lightsout gate manifest --file /tmp/shots.json --min 2); rc=$?
check "one unapproved shot fails a longer manifest" 1 "$rc"
grep -q "shot 2" <<<"$out"
check "the message names the first unapproved shot" 0 "$?"
case "$out" in
  *"shot 1"*) echo "FAIL message names an approved shot too: $out"; fails=$((fails + 1)) ;;
  *) echo "ok   the message leaves the approved shot out" ;;
esac

# a short manifest is a failure even when every entry it has is approved
printf '[{"shot":1,"query":"staff badge","file":"%s","approved":true}]' "$hello" > /tmp/shots.json
out=$(bin/lightsout gate manifest --file /tmp/shots.json --min 2); rc=$?
check "a manifest short of --min fails" 1 "$rc"
case "$out" in
  *"1 shot, want at least 2"*) echo "ok   the failure counts the shots" ;;
  *) echo "FAIL shot count message: $out"; fails=$((fails + 1)) ;;
esac

printf '[{"shot":1,"query":"staff badge","file":"%s","approved":true},{"shot":2,"query":"lobby door","file":"%s","approved":true}]' \
  "$hello" "$hello" > /tmp/shots.json
bin/lightsout gate manifest --file /tmp/shots.json --min 2 >/dev/null
check "two approved shots meet --min" 0 "$?"

# a manifest that cannot be read fails closed
printf 'not json' > /tmp/shots.json
bin/lightsout gate manifest --file /tmp/shots.json --min 1 >/dev/null
check "malformed manifest JSON fails" 1 "$?"
bin/lightsout gate manifest --file "$shots/nope.json" --min 1 >/dev/null
check "a missing manifest fails" 1 "$?"
printf '[]' > /tmp/shots.json
bin/lightsout gate manifest --file /tmp/shots.json >/dev/null
check "an empty manifest fails" 1 "$?"

printf '{"shot":1}\n' > /tmp/shots.json
bin/lightsout gate manifest --file /tmp/shots.json >/dev/null
check "a manifest that is not a list fails" 1 "$?"

rm -rf "$shots" /tmp/shots.json

# 20 — the memory gate: the pass is one gate, and the store it trusts is
# swappable, so a stub can fail save, lint or receipt and the job either blocks
# or completes on the store's verdict (Task 11). HOME is a scratch directory
# with no store in it, so a gate that ignored U_PI_MEMORY would fail rather
# than reach the real store.
fake_memory="$PWD/tests/fake-pi-memory"
memory_home=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-memory-home.XXXXXX")
memory_log=$(mktemp "${TMPDIR:-/tmp}/lightsout-memory-log.XXXXXX")

reset
HOME="$memory_home" U_MEMORY=1 U_PI_MEMORY="$fake_memory" FAKE_MEMORY_RC=1 \
  bin/job run noop >/dev/null 2>&1
check "a failing memory pass blocks" 2 "$?"

reset
HOME="$memory_home" U_MEMORY=1 U_PI_MEMORY="$fake_memory" FAKE_MEMORY_RC=0 \
  bin/job run noop >/dev/null 2>&1
check "a passing memory pass completes" 0 "$?"

# the pass runs save for each named note, then lint, then the receipt
reset
rm -f "$memory_log"
HOME="$memory_home" U_MEMORY=1 U_MEMORY_FILES="notes/00-index.md" \
  U_PI_MEMORY="$fake_memory" FAKE_MEMORY_RC=0 FAKE_MEMORY_LOG="$memory_log" \
  bin/job run noop >/dev/null 2>&1
check "a named note passes save, lint and receipt" 0 "$?"
check "the pass runs all three store scripts" 3 \
  "$(wc -l < "$memory_log" 2>/dev/null | tr -d ' ')"
check "save gets the named note" "qmd-mem save notes/00-index.md" \
  "$(sed -n '1p' "$memory_log" 2>/dev/null)"
check "lint runs deep" "lint --deep" "$(sed -n '2p' "$memory_log" 2>/dev/null)"
check "the receipt is asked for this workspace" "receipt --latest --cwd $PWD" \
  "$(sed -n '3p' "$memory_log" 2>/dev/null)"

# the gate names the step that failed: any one of the three stops the pass
out=$(HOME="$memory_home" U_PI_MEMORY="$fake_memory" FAKE_MEMORY_RC=0 \
  FAKE_MEMORY_FAIL=save bin/lightsout gate memory --files notes/00-index.md 2>&1); rc=$?
check "a save failure fails the memory gate" 1 "$rc"
case "$out" in
  *"save notes/00-index.md failed"*) echo "ok   the gate names the failed save" ;;
  *) echo "FAIL save failure not named: $out"; fails=$((fails + 1)) ;;
esac

out=$(HOME="$memory_home" U_PI_MEMORY="$fake_memory" FAKE_MEMORY_RC=0 \
  FAKE_MEMORY_FAIL=lint bin/lightsout gate memory 2>&1); rc=$?
check "a lint failure fails the memory gate" 1 "$rc"
case "$out" in
  *"lint failed"*) echo "ok   the gate names the failed lint" ;;
  *) echo "FAIL lint failure not named: $out"; fails=$((fails + 1)) ;;
esac

out=$(HOME="$memory_home" U_PI_MEMORY="$fake_memory" FAKE_MEMORY_RC=0 \
  FAKE_MEMORY_FAIL=receipt bin/lightsout gate memory 2>&1); rc=$?
check "a receipt failure fails the memory gate" 1 "$rc"
case "$out" in
  *"receipt failed"*) echo "ok   the gate names the failed receipt" ;;
  *) echo "FAIL receipt failure not named: $out"; fails=$((fails + 1)) ;;
esac

# an unreadable store is a failure, never a skipped check
HOME="$memory_home" U_PI_MEMORY="$fake_memory/missing" FAKE_MEMORY_RC=0 \
  bin/lightsout gate memory >/dev/null 2>&1
check "a missing store fails closed" 1 "$?"

rm -rf "$memory_home" "$memory_log"

# 21 — the agent adapter: one runner per step, a marker that freezes a passing
# result, a failing runner that writes nothing, and no path from `fake` to a
# real model (Task 12). The fixture's agent stage is opt-in (NOOP_AGENT=1),
# because every other check runs the fixture and none of them may spawn a model.
agent_prompt="jobs/noop/state/prompt-$(date +%F).md"
agent_marker="$agent_prompt.done"
agent_calls=$(mktemp "${TMPDIR:-/tmp}/lightsout-agent-calls.XXXXXX")

reset
rm -f "$agent_calls"
NOOP_AGENT=1 U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" bin/job run noop >/dev/null 2>&1
check "a fake-runner agent step completes" 0 "$?"
check "agent invoked once" 1 "$(agent_invocations)"
[ -f "$agent_marker" ]; check "a passing runner writes the marker" 0 "$?"
[ -s "$agent_prompt" ]; check "the prompt is a rendered file" 0 "$?"
[ -f "$agent_prompt.out" ]; check "the agent's artifact reaches the gate" 0 "$?"
NOOP_AGENT=1 U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" bin/job run noop >/dev/null 2>&1
check "a held rerun does not invoke the agent" 1 "$(agent_invocations)"

# a later stage failing leaves the marker behind, so the rerun re-checks the gate
# instead of paying for a second model call
reset
rm -f "$agent_calls"
NOOP_AGENT=1 U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" NOOP_BLOCK=1 \
  bin/job run noop >/dev/null 2>&1
check "a later stage failing blocks the run" 2 "$?"
[ -f "$agent_marker" ]; check "the marker survives a later failure" 0 "$?"
bin/job unblock noop >/dev/null 2>&1
NOOP_AGENT=1 U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" bin/job run noop >/dev/null 2>&1
check "the rerun after a partial failure completes" 0 "$?"
check "a rerun does not pay for the agent again" 1 "$(agent_invocations)"

# a runner that fails writes no marker and does not finish the unit
reset
rm -f "$agent_calls"
NOOP_AGENT=1 U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" FAKE_AGENT_RC=1 \
  bin/job run noop >/dev/null 2>&1
check "a failing runner fails the job" 1 "$?"
check "a failing runner was tried once" 1 "$(agent_invocations)"
[ -f "$agent_marker" ]; check "a failing runner writes no marker" 1 "$?"
[ -f "jobs/noop/state/done/$(date +%F)" ]
check "a failing agent does not finish the unit" 1 "$?"

# u_agent itself: a bare marker lands under $STATE, the second call is free, and
# a runner failure is returned to the job rather than swallowed
agent_direct=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-agent-direct.XXXXXX")
printf 'prompt for the adapter\n' > "$agent_direct/prompt.md"
rm -f "$agent_calls"
out=$(REPO="$PWD" STATE="$agent_direct" LOG="$agent_direct/agent.log" DRY=0 \
  U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" bash -c '
    . "$REPO/lib/guards.sh"
    u_agent "$1/prompt.md" adapter.done; a=$?
    u_agent "$1/prompt.md" adapter.done; b=$?
    printf "%s %s\n" "$a" "$b"' _ "$agent_direct" | tail -1)
check "u_agent is once, then free" "0 0" "$out"
check "the second u_agent call does not invoke the runner" 1 \
  "$(agent_invocations)"
[ -f "$agent_direct/adapter.done" ]; check "a bare marker is written under STATE" 0 "$?"
out=$(REPO="$PWD" STATE="$agent_direct" LOG="$agent_direct/agent.log" DRY=0 \
  U_RUNNER=fake FAKE_AGENT_RC=1 bash -c '
    . "$REPO/lib/guards.sh"
    u_agent "$1/prompt.md" failed.done
    printf "rc=%s\n" "$?"' _ "$agent_direct" 2>/dev/null | tail -1)
check "u_agent returns non-zero on runner failure" "rc=1" "$out"
[ -f "$agent_direct/failed.done" ]; check "a failed u_agent writes no marker" 1 "$?"

# U_RUNNER=fake must run the repository's own stub: decoys named like every
# other runner, and a fake-agent.sh of their own, are never touched, so a fake
# step cannot reach a real model or an arbitrary program on PATH
agent_decoy=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-agent-decoy.XXXXXX")
for c in dsh claude codex fake-agent.sh; do
  printf '#!/bin/sh\nprintf "%%s\\n" "$0" >> "%s/used"\nexit 0\n' "$agent_decoy" \
    > "$agent_decoy/$c"
  chmod +x "$agent_decoy/$c"
done
[ -x tests/fake-agent.sh ]; check "the fake runner is a committed stub" 0 "$?"
reset
rm -f "$agent_calls"
NOOP_AGENT=1 U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" PATH="$agent_decoy:$PATH" \
  bin/job run noop >/dev/null 2>&1
check "the fake runner does not change the verdict" 0 "$?"
[ -f "$agent_decoy/used" ]; check "no runner reachable by PATH is called" 1 "$?"
check "the repository's stub is the one that ran" 1 \
  "$(agent_invocations)"

# an unknown runner is a code error: it fails before anything is frozen
reset
NOOP_AGENT=1 U_RUNNER=not-a-runner bin/job run noop >/dev/null 2>&1
check "an unknown runner fails closed" 1 "$?"
[ -f "$agent_marker" ]; check "an unknown runner writes no marker" 1 "$?"

# plan runs no agent and renders no prompt
reset
rm -f "$agent_calls"
NOOP_AGENT=1 U_RUNNER=fake FAKE_AGENT_LOG="$agent_calls" bin/job plan noop >/dev/null 2>&1
check "plan with an agent stage exits 0" 0 "$?"
check "plan does not invoke the agent" 0 "$(agent_invocations)"
[ -f "$agent_prompt" ]; check "plan renders no prompt" 1 "$?"
[ -f "$agent_marker" ]; check "plan writes no marker" 1 "$?"

rm -rf "$agent_direct" "$agent_decoy" "$agent_calls"

# 22 — the scaffolder: a spec becomes a job whose every stage gate is `never`,
# a spec with a missing heading or a stage without a gate is refused, and an
# existing job is never overwritten (Task 13). The job the test scaffolds is
# deleted when the test finishes, so `bin/job list` is left as it was found.
scaffold_spec=spec/examples/digest.md
scaffold_job=jobs/morning-digest   # the name comes from `# spec:` in the spec
scaffold_bad=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-scaffold.XXXXXX")
rm -rf "$scaffold_job" jobs/no-budget jobs/no-gate-stage

out=$(bin/scaffold "$scaffold_spec" 2>&1); rc=$?
check "scaffold exits 0" 0 "$rc"
[ -x bin/scaffold ]; check "scaffold is executable" 0 "$?"
[ -f "$scaffold_job/job.sh" ]; check "job emitted" 0 "$?"
[ -f "$scaffold_job/spec.md" ]; check "the spec is copied into the job" 0 "$?"
[ -x "$scaffold_job/job.sh" ]; check "the scaffolded job is executable" 0 "$?"
cmp -s "$scaffold_spec" "$scaffold_job/spec.md"
check "the copied spec matches the source" 0 "$?"
check "the job is named from the spec" morning-digest "$(basename "$scaffold_job")"
grep -qx morning-digest <<< "$(bin/job list)"
check "the scaffolded job is listable" 0 "$?"

# every stage becomes a fail-closed `never` gate, and the plan names each one
check "one never gate per stage" 4 "$(grep -c '^u_gate never$' "$scaffold_job/job.sh")"
grep -q '^u_guard_' "$scaffold_job/job.sh"
check "the job calls a guard" 0 "$?"
grep -q '^u_memory_pass' "$scaffold_job/job.sh"
check "the job ends with a memory pass" 0 "$?"
grep -q '^u_done$' "$scaffold_job/job.sh"
check "the job marks the unit done" 0 "$?"
plan_out=$(bin/job plan morning-digest 2>&1); rc=$?
check "plan exits 0 for a scaffolded job" 0 "$rc"
for label in collect summarize write "memory pass"; do
  case "$plan_out" in
    *"$label"*never*) printf 'ok   plan reports the %s gate as never\n' "$label" ;;
    *) printf 'FAIL plan does not report %s: %s\n' "$label" "$plan_out"
       fails=$((fails + 1)) ;;
  esac
done

# an existing job is never overwritten: the scaffold refuses and the old job
# stays byte-identical
job_before=$(md5 -q "$scaffold_job/job.sh")
out=$(bin/scaffold "$scaffold_spec" 2>&1); rc=$?
check "scaffold refuses to overwrite" 64 "$rc"
check "the existing job is untouched" "$job_before" \
  "$(md5 -q "$scaffold_job/job.sh")"
[ -f "$scaffold_job/spec.md" ]; check "the existing spec copy is untouched too" 0 "$?"

# a spec missing one of the nine headings is refused, names the heading, and
# writes nothing
sed -e 's/^# spec: morning-digest$/# spec: no-budget/' \
    -e '/^budget$/,$d' "$scaffold_spec" > "$scaffold_bad/no-budget.md"
out=$(bin/scaffold "$scaffold_bad/no-budget.md" 2>&1); rc=$?
check "scaffold refuses a missing heading" 64 "$rc"
[ -e jobs/no-budget ]; check "a spec with a missing heading writes no job" 1 "$?"
case "$out" in
  *budget*) echo "ok   the refusal names the missing heading" ;;
  *) echo "FAIL missing heading not named: $out"; fails=$((fails + 1)) ;;
esac

# a stage without `gate:` is refused the same way, because a stage nobody can
# judge is exactly what the schema forbids
sed -e 's/^# spec: morning-digest$/# spec: no-gate-stage/' \
    -e 's/gate:.*$//' "$scaffold_spec" > "$scaffold_bad/no-gate.md"
out=$(bin/scaffold "$scaffold_bad/no-gate.md" 2>&1); rc=$?
check "scaffold refuses a stage without a gate" 64 "$rc"
[ -e jobs/no-gate-stage ]; check "a gate-less spec writes no job" 1 "$?"
case "$out" in
  *"no gate"*|*"gate:"*) echo "ok   the refusal names the missing gate" ;;
  *) echo "FAIL missing gate not named: $out"; fails=$((fails + 1)) ;;
esac

# a missing spec file is misuse too, not a crash
bin/scaffold "$scaffold_bad/nope.md" >/dev/null 2>&1
check "scaffold refuses a spec that does not exist" 64 "$?"

# the test cleans up after itself: the scaffolded job is gone, and so is the
# dashboard row it would otherwise add
rm -rf "$scaffold_job" jobs/no-budget jobs/no-gate-stage "$scaffold_bad"
grep -qx morning-digest <<< "$(bin/job list)"
check "the test deletes the job it scaffolded" 1 "$?"

# 23 — the video job: `plan` names the episode the driver would build and every
# gate it would apply, read from the driver's checkout without writing there; a
# run on a fixture driver (no media, no network, no real video repo) goes
# through the agent, the driver and the master gates (Task 14).
video_repo=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-video.XXXXXX")
mkdir -p "$video_repo"/automation "$video_repo"/bin "$video_repo"/episodes \
  "$video_repo"/lessons "$video_repo"/notes "$video_repo"/outbox "$video_repo"/pipeline
video_tree_hash() { find "$1" -type f | sort | xargs md5 -q | md5 -q; }

cat > "$video_repo/lessons/ledger.jsonl" <<'JSON'
{"episode": "041", "title": "A Previous Episode", "principle": "old", "status": "built", "date": "2026-10-19"}
JSON
: > "$video_repo/outbox/041-master.mp4"
cat > "$video_repo/pipeline/topic-queue.txt" <<'TXT'
041|A Previous Episode|OBS. 041 — THE OLD ONE|calm|old
042|Why Unfinished Things Stay With You|OBS. 042 — THE LOOSE THREAD|curious|the mind keeps what is unresolved in view
043|A Later Episode|OBS. 043 — THE NEXT ONE|calm|next
TXT
cat > "$video_repo/automation/lesson-task.md" <<'TXT'
# Daily lesson — {{RUN_DATE}}

Episode **{{EP}}**. Queue line: `{{TOPIC_LINE}}`.
TXT
printf '# notes\n\n## [2026-10-19] build | № 041 "A Previous Episode" built\n' \
  > "$video_repo/notes/60-log.md"

cat > "$video_repo/pipeline/lesson-check.py" <<'PY'
#!/usr/bin/env python3
import sys
print("LESSON-OK")
sys.exit(0)
PY
cat > "$video_repo/pipeline/lesson-split.py" <<'PY'
#!/usr/bin/env python3
import pathlib, sys
ep = pathlib.Path(sys.argv[1])
(ep / "_scratch").mkdir(parents=True, exist_ok=True)
(ep / "_scratch/meta.json").write_text("{}\n")
(ep / "narration").mkdir(parents=True, exist_ok=True)
(ep / "narration/tts.txt").write_text("narration\n")
print("split ok")
PY
chmod +x "$video_repo/pipeline/lesson-check.py" "$video_repo/pipeline/lesson-split.py"

cat > "$video_repo/pipeline/daily-build.sh" <<'SH'
#!/bin/bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
printf '%s\n' "$*" >> "$here/../calls.log"
case "${1:-}" in
  --preflight)
    printf '[fixture] next episode: %s\n' "${FAKE_NEXT_EP:-042}"
    printf '[fixture] preflight OK (ep %s, beds 3)\n' "${FAKE_NEXT_EP:-042}"
    exit 0 ;;
  *)
    if [ "${FAKE_DRIVER_HOLD:-0}" = 1 ]; then
      printf '[fixture] a master was built 3h ago - one per day, skipping\n'
      exit 0
    fi
    : > "$here/../outbox/$1-master.mp4"
    printf '[fixture] built %s\n' "$1"
    exit 0 ;;
esac
SH
chmod +x "$video_repo/pipeline/daily-build.sh"

cat > "$video_repo/bin/ffprobe" <<'SH'
#!/bin/sh
res="${FAKE_VIDEO_RES:-1080x1920}"
w=1080; h=1920
case "$res" in
  *x*) w=${res%x*}; h=${res#*x} ;;
esac
printf '{"format":{"duration":"60.00"},"streams":[{"codec_type":"video","width":%s,"height":%s}]}\n' "$w" "$h"
SH
cat > "$video_repo/bin/ffmpeg" <<'SH'
#!/bin/sh
printf '[Parsed_ebur128_0] Summary:\n  I: -14.0 LUFS\n' >&2
exit 0
SH
chmod +x "$video_repo/bin/ffprobe" "$video_repo/bin/ffmpeg"

rm -rf jobs/everyday-laws/state jobs/everyday-laws/out jobs/everyday-laws/logs
[ -f jobs/everyday-laws/spec.md ] && cmp -s spec/examples/video.md jobs/everyday-laws/spec.md
check "the video spec is a copy of the example" 0 "$?"
grep -qi "first real run is supervised" jobs/everyday-laws/job.sh
check "the job records that its first run is supervised" 0 "$?"

video_tree_before=$(video_tree_hash "$video_repo")
out=$(VIDEO_REPO="$video_repo" bin/job plan everyday-laws 2>&1); rc=$?
check "video plan exits 0" 0 "$rc"
case "$out" in
  *"next episode: 042"*) echo "ok   plan names the next episode" ;;
  *) echo "FAIL plan does not name the next episode: $out"; fails=$((fails + 1)) ;;
esac
for want in "lesson-check.py" "daily-build.sh 042" "duration" "resolution" \
            "loudness" "--min 59" "--max 61" "--w 1080" "--h 1920" \
            "--target -14" "--tol 1" "memory"; do
  case "$out" in
    *"$want"*) ;;
    *) echo "FAIL plan omits the $want gate"; fails=$((fails + 1)) ;;
  esac
done
[ -f "$video_repo/calls.log" ]
check "plan does not execute the driver" 1 "$?"
check "plan writes no state or output" 0 \
  "$(find jobs/everyday-laws/state jobs/everyday-laws/out -type f 2>/dev/null | wc -l | tr -d ' ')"
check "plan writes no job log" 0 \
  "$(find jobs/everyday-laws/logs -type f 2>/dev/null | wc -l | tr -d ' ')"
check "plan writes nothing in the driver's checkout" "$video_tree_before" \
  "$(video_tree_hash "$video_repo")"

# the driver's rule is first-incomplete, then one past the ledger: a plan that
# only read the ledger maximum would answer 045 here
mkdir -p "$video_repo/episodes/043"
printf '{"episode": "044", "title": "Posted Already", "principle": "x", "status": "posted", "date": "2026-10-19"}\n' \
  >> "$video_repo/lessons/ledger.jsonl"
: > "$video_repo/outbox/044-master.mp4"
out=$(VIDEO_REPO="$video_repo" bin/job plan everyday-laws 2>&1)
case "$out" in
  *"next episode: 043"*) echo "ok   plan resumes the unfinished episode first" ;;
  *) echo "FAIL plan did not resume the unfinished episode: $out"; fails=$((fails + 1)) ;;
esac

# a run against the fixture driver: stage 1 through the fake agent, stages 2-8
# through the driver, then the three master gates and the memory pass
video_agent_log=$(mktemp "${TMPDIR:-/tmp}/lightsout-video-agent.XXXXXX")
rm -rf jobs/everyday-laws/state jobs/everyday-laws/out jobs/everyday-laws/logs \
  "$video_repo/calls.log"
rm -f "$video_repo"/outbox/*-master.mp4
PATH="$video_repo/bin:$PATH" VIDEO_REPO="$video_repo" U_RUNNER=fake \
  FAKE_AGENT_LOG="$video_agent_log" \
  FAKE_AGENT_OUT="$video_repo/episodes/042/lesson-raw.md" \
  U_MEMORY=1 U_PI_MEMORY="$PWD/tests/fake-pi-memory" \
  bin/job run everyday-laws >/dev/null 2>&1
check "the fixture video run exits 0" 0 "$?"
check "the agent ran stage 1 once" 1 \
  "$(wc -l < "$video_agent_log" 2>/dev/null | tr -d ' ')"
check "the driver was asked to preflight and build" 2 \
  "$(wc -l < "$video_repo/calls.log" 2>/dev/null | tr -d ' ')"
check "the preflight comes first" "--preflight" \
  "$(sed -n '1p' "$video_repo/calls.log" 2>/dev/null)"
check "the build gets the episode number" "042" \
  "$(sed -n '2p' "$video_repo/calls.log" 2>/dev/null)"
[ -f "$video_repo/outbox/042-master.mp4" ]
check "the master is on disk" 0 "$?"
[ -f "jobs/everyday-laws/state/done/$(date +%F)" ]
check "a passing master marks the day done" 0 "$?"

# a master the resolution gate rejects blocks the job and does not mark the day
rm -rf jobs/everyday-laws/state jobs/everyday-laws/out jobs/everyday-laws/logs \
  "$video_repo/calls.log"
rm -f "$video_repo/outbox/042-master.mp4"
PATH="$video_repo/bin:$PATH" VIDEO_REPO="$video_repo" U_RUNNER=fake \
  FAKE_AGENT_LOG="$video_agent_log" \
  FAKE_AGENT_OUT="$video_repo/episodes/042/lesson-raw.md" FAKE_VIDEO_RES=720x1280 \
  U_MEMORY=1 U_PI_MEMORY="$PWD/tests/fake-pi-memory" \
  bin/job run everyday-laws >/dev/null 2>&1
check "a master that is not 1080x1920 blocks" 2 "$?"
[ -f jobs/everyday-laws/state/blocked ]
check "the bad master leaves a block" 0 "$?"
grep -q "resolution" jobs/everyday-laws/state/blocked 2>/dev/null
check "the block names the resolution gate" 0 "$?"
[ -f "jobs/everyday-laws/state/done/$(date +%F)" ]
check "a blocked master does not mark the day done" 1 "$?"

# a driver that decides not to build must hold, never block: a block would
# refuse every later run until a human cleared it
rm -rf jobs/everyday-laws/state jobs/everyday-laws/out jobs/everyday-laws/logs \
  "$video_repo/calls.log"
rm -f "$video_repo"/outbox/042-master.mp4
PATH="$video_repo/bin:$PATH" VIDEO_REPO="$video_repo" U_RUNNER=fake \
  FAKE_AGENT_LOG="$video_agent_log" \
  FAKE_AGENT_OUT="$video_repo/episodes/042/lesson-raw.md" FAKE_DRIVER_HOLD=1 \
  U_MEMORY=1 U_PI_MEMORY="$PWD/tests/fake-pi-memory" \
  bin/job run everyday-laws >/dev/null 2>&1
check "a driver hold exits 0" 0 "$?"
[ -f jobs/everyday-laws/state/blocked ]
check "a driver hold leaves no block" 1 "$?"
[ -f "jobs/everyday-laws/state/done/$(date +%F)" ]
check "a driver hold does not mark the day done" 1 "$?"

# and a master already on disk today is asked before a single attempt is spent
rm -rf jobs/everyday-laws/state jobs/everyday-laws/out jobs/everyday-laws/logs \
  "$video_repo/calls.log"
: > "$video_repo/outbox/041-master.mp4"
PATH="$video_repo/bin:$PATH" VIDEO_REPO="$video_repo" U_RUNNER=fake \
  FAKE_AGENT_LOG="$video_agent_log" \
  FAKE_AGENT_OUT="$video_repo/episodes/042/lesson-raw.md" \
  U_MEMORY=1 U_PI_MEMORY="$PWD/tests/fake-pi-memory" \
  bin/job run everyday-laws >/dev/null 2>&1
check "a same-day master holds" 0 "$?"
check "the hold spends no attempt" 0 \
  "$(find jobs/everyday-laws/state/attempts -type f 2>/dev/null | wc -l | tr -d ' ')"
[ -f "$video_repo/calls.log" ]
check "the hold never starts the driver" 1 "$?"

rm -rf "$video_repo" "$video_agent_log" jobs/everyday-laws/state \
  jobs/everyday-laws/out jobs/everyday-laws/logs
reset
echo
if [ "$fails" -eq 0 ]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
