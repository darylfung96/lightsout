#!/usr/bin/env bash
# everyday-laws — the Everyday Laws video job, ported from ../video-automation.
#
# This job is the clock the launchd daily build has been missing: the same
# pipeline, with LightsOut's day guard, lock, attempt cap, disk guard and gates
# in front of it. Stage 1 (the lesson) goes through u_agent; stages 2-8 stay in
# the proven pipeline/daily-build.sh driver, called with the episode number so
# it resumes rather than chooses. The master is the deliverable, so its
# duration, resolution and loudness are judged here even though the driver
# already measured them: only a gate may pass a gate.
#
# ** THE FIRST REAL RUN IS SUPERVISED. ** This job has not yet built an episode
# from LightsOut. Watch one full run end to end — preflight, agent, driver,
# master gates, memory pass, done — before scheduling it. For a build-only
# rehearsal set AUTOPOST=0 (the driver then leaves the episode built and
# unposted); set U_MEMORY=0 to rehearse without touching ~/agent-memory.
#
# VIDEO_REPO is the driver's checkout (default ~/programming/video-automation).
# `bin/job plan` answers the episode question from that checkout read-only: it
# must not write there or start the driver's services. A real run asks
# `daily-build.sh --preflight`, so the driver still owns the episode rule.
#
# Contract: docs/run-contract.md. Spec: jobs/everyday-laws/spec.md.
set -euo pipefail
. "$REPO/lib/guards.sh"
. "$REPO/lib/memory.sh"

VIDEO_REPO="${VIDEO_REPO:-$HOME/programming/video-automation}"
case "$VIDEO_REPO" in
  /*) ;;
  *)  VIDEO_REPO="$PWD/$VIDEO_REPO" ;;
esac
[ -d "$VIDEO_REPO" ] || {
  printf 'everyday-laws: VIDEO_REPO is not a directory: %s\n' "$VIDEO_REPO" >&2
  exit 2
}
[ -x "$VIDEO_REPO/pipeline/daily-build.sh" ] || {
  printf 'everyday-laws: no executable driver at %s\n' \
    "$VIDEO_REPO/pipeline/daily-build.sh" >&2
  exit 2
}

# plan touches nothing: u_say's job log is the one file it would create, so in
# plan mode the report goes to stdout only (docs/run-contract.md).
if [ "$DRY" = 1 ]; then LOG=/dev/null; fi

# The memory pass is the last stage and it is mandatory for this job; a
# rehearsal may set U_MEMORY=0, which lib/memory.sh reports as skipped.
U_MEMORY="${U_MEMORY:-1}"
U_MEMORY_FILES="${U_MEMORY_FILES:-notes/60-log.md}"
export U_MEMORY U_MEMORY_FILES

# The dsh patch pins the provider and model for the lesson agent; without it a
# headless run depends on GUI session settings (automation/README.md).
if [ -z "${U_AGENT_PATCH:-}" ] && [ -f "$VIDEO_REPO/automation/headless.patch.yml" ]; then
  U_AGENT_PATCH="$VIDEO_REPO/automation/headless.patch.yml"
fi

# --- the driver's episode rule, read-only ---------------------------------
# plan may not execute the driver (its preflight starts services and appends to
# its own log), so this is the driver's rule transcribed: skip episodes that
# have a master or are posted, take the first unfinished one, else one past the
# ledger's highest episode. A real run still asks the driver; the fixture test
# in tests/run.sh holds this read-only answer to the driver's own rule.
next_episode_readonly() {
  python3 - "$VIDEO_REPO" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
ledger = root / "lessons" / "ledger.jsonl"
try:
    rows = [json.loads(line) for line in ledger.read_text(encoding="utf-8").splitlines()
            if line.strip()]
except OSError as exc:
    print(f"cannot read {ledger}: {exc}", file=sys.stderr)
    raise SystemExit(1)
posted = {str(row.get("episode")) for row in rows if row.get("status") == "posted"}
episodes = sorted(path.name for path in (root / "episodes").glob("[0-9][0-9][0-9]"))
for name in episodes:
    if (root / "outbox" / f"{name}-master.mp4").exists():
        continue
    if name in posted:
        continue
    print(name)
    raise SystemExit(0)
try:
    newest = max(int(str(row["episode"])) for row in rows)
except (KeyError, ValueError) as exc:
    print(f"{ledger}: cannot read an episode number ({exc})", file=sys.stderr)
    raise SystemExit(1)
print(f"{newest + 1:03d}")
PY
}

# --- the driver's preflight, for a real run -------------------------------
# The driver owns the rule, so a run asks it rather than trusting the read-only
# copy above. Its output is copied into the job log because the job log is
# where a human looks after a stop. "another build is running" is a hold, not a
# failure; every other preflight failure needs a human.
PREFLIGHT_EP=""
preflight() {
  local out rc last ep
  set +e
  out=$("$VIDEO_REPO/pipeline/daily-build.sh" --preflight 2>&1)
  rc=$?
  set -e
  if [ -n "$out" ]; then printf '%s\n' "$out" >> "$LOG"; fi
  if [ "$rc" -ne 0 ]; then
    if printf '%s\n' "$out" | grep -q "another build is running"; then
      u_held "the driver is already building (logs/daily-build.lock)"
    fi
    last=$(printf '%s\n' "$out" | tail -1)
    u_block --class human --source preflight \
      "preflight failed (rc $rc): ${last:-no output}"
  fi
  ep=$(printf '%s\n' "$out" | sed -n 's/.*next episode: \([0-9][0-9][0-9]\).*/\1/p' | tail -1)
  [ -n "$ep" ] || u_block --class human --source preflight \
    "cannot parse the next episode from the preflight output"
  PREFLIGHT_EP="$ep"
}

# --- stage 1's prompt -----------------------------------------------------
render_prompt() { # render_prompt <out> <episode> <queue-line>
  python3 - "$VIDEO_REPO/automation/lesson-task.md" "$1" "$2" "$3" "$(date +%F)" <<'PY'
import sys

template, out, episode, topic, run_date = sys.argv[1:6]
text = open(template, encoding="utf-8").read()
open(out, "w", encoding="utf-8").write(
    text.replace("{{EP}}", episode)
        .replace("{{TOPIC_LINE}}", topic)
        .replace("{{RUN_DATE}}", run_date))
PY
}

# --- guards ---------------------------------------------------------------
# Cheap first: a tick that cannot work must not start the driver's services.
u_guard_day
u_guard_lock
# The driver has its own build lock, and a tick that found another build there
# did not try: hold before the attempt is counted, so a transition overlap with
# the launchd job costs no attempt budget (docs/run-contract.md).
if [ -d "$VIDEO_REPO/logs/daily-build.lock" ]; then
  u_held "the driver's build lock exists ($VIDEO_REPO/logs/daily-build.lock)"
fi
# The driver's rule is one build per day, measured from the newest master, and a
# tick that cannot build must spend no attempt: ask that rule here, before the
# attempt file exists (docs/run-contract.md, "guards").
if [ "$DRY" = 0 ]; then
  newest_master=$(ls -t "$VIDEO_REPO"/outbox/[0-9][0-9][0-9]-master.mp4 2>/dev/null | head -1 || true)
  if [ -n "$newest_master" ]; then
    master_age_h=$(( ($(date +%s) - $(stat -f %m "$newest_master")) / 3600 ))
    if [ "$(date -r "$newest_master" +%F)" = "$(date +%F)" ] || [ "$master_age_h" -lt 20 ]; then
      u_held "a master was built ${master_age_h}h ago (${newest_master##*/}) — one per day"
    fi
  fi
fi
u_guard_attempts
u_guard_disk 2000

if [ "$DRY" = 1 ]; then
  NEXT_EP=$(next_episode_readonly) || {
    printf 'everyday-laws: cannot resolve the next episode from %s\n' "$VIDEO_REPO" >&2
    exit 2
  }
  ep="$NEXT_EP"
else
  preflight
  ep="$PREFLIGHT_EP"
fi
[ -n "$ep" ] || { printf 'everyday-laws: no episode resolved\n' >&2; exit 2; }

topic=$(grep -E "^${ep}\|" "$VIDEO_REPO/pipeline/topic-queue.txt" 2>/dev/null | head -1 || true)
[ -n "$topic" ] || u_block --class human --source preflight \
  "REFILL-TOPICS: no queue line for episode $ep"
IFS='|' read -r _queue title obs _tone _angle <<< "$topic"

master="$VIDEO_REPO/outbox/$ep-master.mp4"
lesson_dir="$VIDEO_REPO/episodes/$ep"
prompt="$STATE/prompt-$ep.md"
marker="$prompt.done"

# The three files the driver resumes stage 1 from. Any one missing means the
# stage has not finished, so the lesson is re-checked on every attempt.
stage1_staged() {
  [ -s "$lesson_dir/lesson-raw.md" ] \
    && [ -s "$lesson_dir/_scratch/meta.json" ] \
    && [ -s "$lesson_dir/narration/tts.txt" ]
}

if [ "$DRY" = 1 ]; then
  u_say "next episode: $ep"
  u_say "topic: $title ($obs)"
  u_say "plan: stage 1: render $prompt, run agent (${U_RUNNER:-dsh}), gate $VIDEO_REPO/pipeline/lesson-check.py"
  u_say "plan: stages 2-8: $VIDEO_REPO/pipeline/daily-build.sh $ep"
  u_say "plan: gate duration --file $master --min 59 --max 61"
  u_say "plan: gate resolution --file $master --w 1080 --h 1920"
  u_say "plan: gate loudness --file $master --target -14 --tol 1"
  u_say "plan: gate memory --files $U_MEMORY_FILES"
  u_done
  u_say "DONE"
  exit 0
fi

u_say "next episode: $ep"
u_say "topic: $title ($obs)"
cd "$VIDEO_REPO"
mkdir -p "$lesson_dir/narration" "$lesson_dir/_scratch"

# --- stage 1: the lesson ---------------------------------------------------
# The agent marker freezes the paid call, never the verdict: the checker runs
# on every attempt, outside any marker test.
if ! stage1_staged && [ ! -s "$lesson_dir/lesson-raw.md" ]; then
  render_prompt "$prompt" "$ep" "$topic" \
    || u_block --class human --source stage:1 \
       "cannot render $VIDEO_REPO/automation/lesson-task.md"
  u_say "stage 1: prompt rendered ($prompt)"
  u_agent "$prompt" "$marker" \
    || u_block --class human --source stage:1 "lesson agent failed"
elif ! stage1_staged; then
  u_say "stage 1: lesson-raw.md present; resuming at the split"
fi
u_say "stage 1: lesson gate"
python3 "$VIDEO_REPO/pipeline/lesson-check.py" "$lesson_dir" \
  || u_block --class judgment --source stage:1 \
     "lesson failed pipeline/lesson-check.py"
if ! stage1_staged; then
  python3 "$VIDEO_REPO/pipeline/lesson-split.py" "$lesson_dir" \
    || u_block --class judgment --source stage:1 "pipeline/lesson-split.py failed"
fi
stage1_staged \
  || u_block --class judgment --source stage:1 \
     "stage 1 did not stage lesson-raw.md, meta.json and tts.txt"

# --- stages 2-8: the deterministic driver ---------------------------------
u_say "stages 2-8: daily-build.sh $ep"
set +e
"$VIDEO_REPO/pipeline/daily-build.sh" "$ep" 2>&1 | tee -a "$LOG"
build_rc=${PIPESTATUS[0]}
set -e
if [ "$build_rc" -ne 0 ]; then
  u_block --class judgment --source stage:2-8 \
    "daily-build.sh $ep failed (rc $build_rc); see $VIDEO_REPO/logs/daily-build-$(date +%F).log"
fi

# A driver that exits 0 without a master decided not to build (its own day
# guard, or an episode it judged already done). That is a hold, not a verdict:
# blocks are permanent until a human clears them, and a false one would stop
# every later run.
if [ ! -f "$master" ]; then
  u_held "the driver exited 0 without $ep's master — it decided not to build"
fi

# --- the master is the deliverable: judge what the driver produced ---------
u_say "stage 7: master gates on $master"
U_BLOCK_CLASS=judgment u_gate duration --file "$master" --min 59 --max 61
U_BLOCK_CLASS=judgment u_gate resolution --file "$master" --w 1080 --h 1920
U_BLOCK_CLASS=judgment u_gate loudness --file "$master" --target -14 --tol 1

# --- the last stage: the memory pass --------------------------------------
u_say "stage 9: memory pass"
u_memory_pass || u_block "memory pass failed"

u_done
u_say "DONE: episode $ep — $title"
