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
[ -d "$VIDEO_REPO" ] || u_block --class human --source preflight \
  "VIDEO_REPO is not a directory: $VIDEO_REPO"
[ -x "$VIDEO_REPO/pipeline/daily-build.sh" ] || u_block --class human --source preflight \
  "no executable driver at $VIDEO_REPO/pipeline/daily-build.sh"

# plan touches nothing: u_say's job log is the one file it would create, so in
# plan mode the report goes to stdout only (docs/run-contract.md).
if [ "$DRY" = 1 ]; then LOG=/dev/null; fi

# The memory pass is the last stage and it is mandatory for this job; a
# rehearsal may set U_MEMORY=0, which lib/memory.sh reports as skipped.
U_MEMORY="${U_MEMORY:-1}"
U_MEMORY_FILES="${U_MEMORY_FILES:-notes/60-log.md}"
export U_MEMORY U_MEMORY_FILES

# Stage 1's lesson is the only bounded step in this job: stages 2-8 are one
# driver call, and the manager waits for a job however long it runs, so a video
# build over an hour is never killed by a timeout. An hour is the ceiling for
# the text call alone, because a slow provider should not turn a build into a
# blocked job; U_AGENT_TIMEOUT in the environment still wins.
U_AGENT_TIMEOUT="${U_AGENT_TIMEOUT:-3600}"
export U_AGENT_TIMEOUT

# The queue used to be the one human input. Stage 0 tops it up: when the next
# episode has no line, the agent proposes TOPIC_BUFFER lines into a proposal
# file and pipeline/topic_check.py appends them only after the format, the
# numbering and the no-duplicate rule pass. A line that already exists costs no
# agent call.
TOPIC_BUFFER="${TOPIC_BUFFER:-3}"
case "$TOPIC_BUFFER" in
  ''|*[!0-9]*) u_die "TOPIC_BUFFER must be a whole number" ;;
esac
[ "$TOPIC_BUFFER" -ge 1 ] && [ "$TOPIC_BUFFER" -le 9 ] \
  || u_die "TOPIC_BUFFER must be 1-9"
TOPIC_QUEUE="${TOPIC_QUEUE:-$VIDEO_REPO/pipeline/topic-queue.txt}"
TOPIC_PROPOSAL="${TOPIC_PROPOSAL:-$VIDEO_REPO/pipeline/topic-proposal.txt}"

# jobs/everyday-laws/agent.conf pins the same dsh patch for the lesson and for
# the repair path; this fallback keeps a direct job.sh invocation working when
# the manager is not in the loop. Without the patch a headless run depends on
# GUI session settings (automation/README.md).
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

# --- stage 0: the topic queue refills itself ------------------------------
render_topics_prompt() { # render_topics_prompt <out> <start-ep> <count>
  python3 - "$VIDEO_REPO/automation/topic-task.md" "$1" "$2" "$3" \
    "$TOPIC_PROPOSAL" "$TOPIC_QUEUE" "$(date +%F)" <<'PY'
import sys
from pathlib import Path

template, out, start, count, proposal, queue, run_date = sys.argv[1:8]
text = Path(template).read_text(encoding="utf-8")
tail = "\n".join(Path(queue).read_text(encoding="utf-8").splitlines()[-10:])
Path(out).write_text(
    text.replace("{{RUN_DATE}}", run_date)
        .replace("{{START_EP}}", start)
        .replace("{{COUNT}}", count)
        .replace("{{OUT}}", proposal)
        .replace("{{QUEUE_TAIL}}", tail),
    encoding="utf-8")
PY
}

ensure_topics() { # ensure_topics <episode>
  local ep="$1" later prompt marker
  if grep -q "^${ep}|" "$TOPIC_QUEUE" 2>/dev/null; then
    return 0
  fi
  if [ "$DRY" = 1 ]; then
    u_say "plan: would ask the agent for $TOPIC_BUFFER topic lines from $ep"
    return 0
  fi
  # Appending is only safe at the end of the queue: a missing middle number is
  # a queue that needs a human, not a place to insert.
  later=$(sed -n 's/^\([0-9][0-9][0-9]\)|.*/\1/p' "$TOPIC_QUEUE" 2>/dev/null \
    | awk -v ep="$ep" '$1+0 > ep+0 { print; exit }')
  [ -z "$later" ] || u_block --class human --source topics \
    "topic queue has a gap at episode $ep (episode $later exists)"
  prompt="$STATE/topics-$ep.md"
  marker="$prompt.done"
  render_topics_prompt "$prompt" "$ep" "$TOPIC_BUFFER" \
    || u_block --class human --source topics \
       "cannot render $VIDEO_REPO/automation/topic-task.md"
  rm -f "$TOPIC_PROPOSAL"
  u_say "stage 0: no topic line for $ep; asking for $TOPIC_BUFFER"
  ( cd "$VIDEO_REPO" && u_agent "$prompt" "$marker" ) \
    || u_block --class human --source topics "topic agent failed"
  python3 "$VIDEO_REPO/pipeline/topic_check.py" \
    "$TOPIC_PROPOSAL" "$TOPIC_QUEUE" "$ep" "$TOPIC_BUFFER" --append \
    || u_block --class judgment --source topics \
       "topic proposal failed pipeline/topic_check.py"
  grep -q "^${ep}|" "$TOPIC_QUEUE" \
    || u_block --class human --source topics \
       "topic queue still has no line for $ep after the refill"
  rm -f "$TOPIC_PROPOSAL"
  u_say "topics: added $TOPIC_BUFFER lines from episode $ep"
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
  NEXT_EP=$(next_episode_readonly) || u_block --class human --source preflight \
    "cannot resolve the next episode from $VIDEO_REPO"
  ep="$NEXT_EP"
else
  preflight
  ep="$PREFLIGHT_EP"
fi
[ -n "$ep" ] || u_block --class human --source preflight "no episode resolved"

ensure_topics "$ep"

topic=$(grep -E "^${ep}\|" "$TOPIC_QUEUE" 2>/dev/null | head -1 || true)
if [ -n "$topic" ]; then
  IFS='|' read -r _queue title obs _tone _angle <<< "$topic"
elif [ "$DRY" = 1 ]; then
  # A read-only plan cannot run the agent; show the shape it would fill in.
  title="(generated at run time)"
  obs="OBS. $ep — THE <NOUN>"
else
  u_block --class human --source preflight \
    "REFILL-TOPICS: no queue line for episode $ep"
fi

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
