# LightsOut: close the gaps — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn LightsOut from a contract that records failures into a manager that classifies, diagnoses, and safely repairs them — while making the memory pass an enforced gate rather than a documented habit.

**Architecture:** Three additions on top of the existing six verbs: a failure *class* recorded in `state/blocked` (retryable | judgment | human), a read-only `diagnose` verb that writes `state/diagnosis.md`, and an allowlisted `retry` verb that may only run repairs, never pass a gate. Everything else is accounting and visibility layered on the same files: a run ledger, a spend ledger, per-stage attempt caps, resource locks, stale-run detection, and a memory gate. No database, no server, no new dependency.

**Tech Stack:** bash 3.2 (macOS system shell), Python 3 stdlib (`lib/gates.py`, `lib/report.py`), launchd/cron as the external clock (unchanged), `~/pi-memory` CLI as the memory store.

**Spec:** `docs/design.md` (why), `docs/run-contract.md` (the interface), `docs/memory-pass.md` (the memory rules). The gap list this plan closes is the "Gaps addressed" table below.

## Gaps addressed

| # | gap from the review | task |
|---|---|---|
| 1 | failures are measured, not diagnosed | 2 |
| 2 | no safe repair, only a manual unblock | 3 |
| 3 | attempt caps are per day, not per stage | 4 |
| 4 | spend is invisible | 5 |
| 5 | a job that never runs looks fine | 6 |
| 6 | notifications only reach the desktop | 7 |
| 7 | no run history or durations | 8 |
| 8 | no cross-job concurrency control | 9 |
| 9 | footage quality has no gate anywhere | 10 |
| 10 | the memory pass is documented, not enforced | 11 |
| 11 | no agent adapter, so each job copies the wrapper | 12 |
| 12 | a new job is hand-built, so the pattern drifts | 13 |
| 13 | the video job has not been ported | 14 |

## Global Constraints

- macOS ships bash 3.2: no associative arrays, no `${var,,}`, no `wait -n`.
- Python 3.9+, standard library only. No new packages.
- Exit codes stay `0` done or held, `1` failed, `2` blocked. Every new verb obeys them.
- State stays files under `jobs/<name>/state/`. No database, no daemon.
- Gates fail closed: an unimplemented check is `gates.py never`.
- A repair may change the world, never the verdict. Nothing except a gate may mark a gate passed.
- The last stage of every job is the memory pass; its failure blocks the job.
- `tests/run.sh` must be green at the end of every task, and each task adds its own checks to it.
- Commit per task, one commit, message `type: what changed`.

## Review Focus

1. **Diagnosis changes nothing.** After `job diagnose` on a blocked job, `state/blocked` is byte-identical and the job still exits `2`. Test added in Task 2.
2. **Retry cannot launder a bad artifact.** A `judgment`-class block refuses `job retry`, and a retryable repair still has to pass the same gate afterwards. Test added in Task 3.
3. **Spend is not double-counted** when a run is retried after a partial failure. Test added in Task 5.
4. **A legitimate hold is not stale.** A job held by the day guard or the lock must never be reported as a missed run. Test added in Task 6.
5. **A resource lock left by SIGKILL** must be reclaimed by the next run, not deadlock it. Test added in Task 9.

---

### Task 1: Failure classification

**Files:**
- Modify: `lib/guards.sh` (`u_block`)
- Modify: `bin/job` (blocked short-circuit output)
- Modify: `lib/report.py` (`job_state`, add `class` + `source`)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: nothing new.
- Produces: `state/blocked` is TSV `<iso>\t<class>\t<source>\t<reason>`, `class` ∈ `retryable|judgment|human`, defaults `human`, `source` defaults `unknown`. Two-field files from an older run are read as `human` with `reason` = field 2. `u_block [--class C] [--source S] <reason>`.

- [ ] **Step 1: Write the failing test** in `tests/run.sh`, after the existing blocked scenario:

```bash
reset
NOOP_BLOCK=1 bin/job run noop --class retryable >/dev/null 2>&1
row=$(bin/job status --all --json | python3 -c 'import json,sys; print([r for r in json.load(sys.stdin) if r["name"]=="noop"][0]["class"])')
check "block class is recorded" retryable "$row"
```

- [ ] **Step 2: Run it and watch it fail**

Run: `./tests/run.sh`
Expected: FAIL — `class` is `None` / KeyError.

- [ ] **Step 3: Implement** — `u_block` parses optional `--class`/`--source`, writes four fields; `report.py` splits four fields and exposes `class` and `source`; `bin/job` prints `blocked since <iso> [<class>]: <reason>` and names `bin/job retry` only for `retryable`.

- [ ] **Step 4: Run the tests**

Run: `./tests/run.sh`
Expected: all checks pass, including the new one.

- [ ] **Step 5: Commit** `git commit -m "feat: classify blocked failures"`

---

### Task 2: Diagnose (read-only)

**Files:**
- Create: `lib/diagnose.py`
- Modify: `bin/job` (add `diagnose` verb)
- Modify: `docs/run-contract.md` (verb table)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: `state/blocked` (Task 1), `$LOG`, `$STATE`, `$OUT`.
- Produces: `state/diagnosis.md`, written by `diagnose.py <job-dir>`; `bin/job diagnose <name>` prints it and exits `0` even when blocked. `diagnose.py` never writes anything except `state/diagnosis.md`.

- [ ] **Step 1: Write the failing test**

```bash
reset
NOOP_BLOCK=1 bin/job run noop >/dev/null 2>&1
before=$(md5 -q jobs/noop/state/blocked)
bin/job diagnose noop >/dev/null; check "diagnose exits 0" 0 "$?"
after=$(md5 -q jobs/noop/state/blocked)
check "diagnose leaves the block untouched" "$before" "$after"
[ -s jobs/noop/state/diagnosis.md ]; check "diagnosis written" 0 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — `bin/job diagnose` is an unknown verb.

- [ ] **Step 3: Implement** — `diagnose.py` reads the blocked line, the last 40 log lines before the block, the last failing `FAIL ` line, and lists `$OUT` with sizes; writes a markdown report with sections Cause candidates / Evidence / Next commands, chosen from the class: retryable → the allowlist from Task 3; judgment → "re-run the gate after editing the artifact"; human → "check credentials/disk, then unblock".

- [ ] **Step 4: Run the tests** — `./tests/run.sh` passes.

- [ ] **Step 5: Commit** `git commit -m "feat: add read-only diagnose verb"`

---

### Task 3: Retry allowlist

**Files:**
- Create: `lib/repairs.sh`
- Modify: `bin/job` (add `retry` verb)
- Modify: `jobs/noop/job.sh` (fixture repair `touch-missing`)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: block class (Task 1).
- Produces: `bin/job retry <name>`; repairs registered as `repair_<name>()` in `lib/repairs.sh`; `state/repairs/<date>.count` capped by `MAX_REPAIRS` (default 2). Exits `2` and touches nothing when the class is not `retryable` or the repair is unknown.

- [ ] **Step 1: Write the failing test**

```bash
reset
NOOP_BLOCK=1 NOOP_CLASS=retryable bin/job run noop >/dev/null 2>&1
bin/job retry noop >/dev/null 2>&1; check "retry runs after a retryable block" 0 "$?"
reset
NOOP_BLOCK=1 NOOP_CLASS=human bin/job run noop >/dev/null 2>&1
bin/job retry noop >/dev/null 2>&1; check "retry refuses a human block" 2 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — `retry` is an unknown verb.

- [ ] **Step 3: Implement** — `retry` reads the class; if not `retryable`, print the diagnosis path and exit `2`; else run the named repair from `NOOP_REPAIR` (default `touch-missing`), bump the repair count, then call `run_job` unchanged so the gate still decides.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: allowlisted retry for retryable failures"`

---

### Task 4: Per-stage attempt caps

**Files:**
- Modify: `lib/guards.sh` (`u_guard_stage`)
- Modify: `jobs/noop/job.sh` (stage guard around stage 1)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: `$STATE/attempts`.
- Produces: `u_guard_stage <id>`; files `state/attempts/<date>.<id>.attempt-N`; cap `MAX_STAGE_ATTEMPTS` (default 2); blocks with class `human` on the third.

- [ ] **Step 1: Write the failing test**

```bash
reset
for i in 1 2 3; do NOOP_FAIL_STAGE=1 bin/job run noop >/dev/null 2>&1; done
bin/job status --all --json | python3 -c '
import json,sys
r=[x for x in json.load(sys.stdin) if x["name"]=="noop"][0]
assert r["state"]=="blocked" and "stage" in r["detail"], r'
```

- [ ] **Step 2: Run it and watch it fail** — the third run is stopped by the day-level cap, not the stage cap, so the detail is wrong.

- [ ] **Step 3: Implement** — `u_guard_stage` counts only its own stage files; `noop` calls it at the top of stage 1 and honours `NOOP_FAIL_STAGE=1` by calling `u_block --class human "stage 1 failed (fixture)"` before writing the artifact.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: per-stage attempt caps"`

---

### Task 5: Spend ledger

**Files:**
- Modify: `lib/guards.sh` (`u_spend`, `u_guard_spend`)
- Modify: `lib/report.py` (`spend_today`)
- Modify: `jobs/noop/job.sh` (fixture spend of 25c)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: `state/ledger.tsv` lines `<iso>\t<cents>\t<label>\t<unit>`; `u_spend <cents> <label>`; `u_guard_spend <max_cents>` reads only today's lines; `status --json` gains `spend_today`.

- [ ] **Step 1: Write the failing test**

```bash
reset
bin/job run noop >/dev/null 2>&1
bin/job status --all --json | python3 -c '
import json,sys
r=[x for x in json.load(sys.stdin) if x["name"]=="noop"][0]
assert r["spend_today"]==25, r'
reset
U_SPEND_CAP=10 bin/job run noop >/dev/null 2>&1; check "spend cap blocks" 2 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — `spend_today` is missing.

- [ ] **Step 3: Implement** — `u_spend` appends with `date +%F`; `u_guard_spend` sums today's column 2 with awk; `report.py` sums the same file; the fixture spends 25 cents once per unit, guarded by `state/done`, so a retry does not double-count.

- [ ] **Step 4: Run the tests** — passes, including the no-double-count case.

- [ ] **Step 5: Commit** `git commit -m "feat: spend ledger and cap"`

---

### Task 6: Stale-run detection

**Files:**
- Modify: `lib/report.py` (add `stale`)
- Modify: `bin/job` (add `check` verb)
- Modify: `docs/run-contract.md`
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: `state/last`, `state/done`, `state/lock`.
- Produces: `state/expect` containing `HH:MM`; `job_status.stale` is true when now is past `HH:MM`, no `done/<today>`, no lock, and no blocked marker; `bin/job check` notifies once per job per day (`state/alerts/<date>.stale`) and exits `0` unless a job is both stale and blocked.

- [ ] **Step 1: Write the failing test**

```bash
reset
printf '00:00\n' > jobs/noop/state/expect
bin/job status --all --json | python3 -c '
import json,sys
r=[x for x in json.load(sys.stdin) if x["name"]=="noop"][0]
assert r["stale"] is True, r'
bin/job run noop >/dev/null 2>&1
bin/job status --all --json | python3 -c '
import json,sys
r=[x for x in json.load(sys.stdin) if x["name"]=="noop"][0]
assert r["stale"] is False, r'
```

- [ ] **Step 2: Run it and watch it fail** — no `stale` key.

- [ ] **Step 3: Implement** — compute from files only; a held run writes `state/last` with rc 0 and sets `state/held`, which suppresses stale for the rest of that day.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: detect a run that never happened"`

---

### Task 7: Notification channels

**Files:**
- Create: `lib/notify.sh`
- Modify: `lib/guards.sh` (delegate `u_notify`)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: `U_NOTIFY` (0/1), `U_ALERTS`.
- Produces: `u_notify <title> <body>` tries each configured channel (`U_NOTIFY_CMD`, `U_NOTIFY_WEBHOOK`, `osascript`) with a 5-second timeout, always appends to `$U_ALERTS`, and never fails the job.

- [ ] **Step 1: Write the failing test**

```bash
reset
NOOP_BLOCK=1 U_NOTIFY_CMD='printf "%s|%s\n" >> /tmp/lightsout-notify' bin/job run noop >/dev/null 2>&1
grep -q "noop blocked" /tmp/lightsout-notify; check "notify cmd fired" 0 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — `U_NOTIFY_CMD` is ignored.

- [ ] **Step 3: Implement** — channel loop; `command -v` before use; `timeout` unavailable on macOS, so use `perl -e 'alarm 5; exec @ARGV'` or background-plus-kill.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: pluggable notification channels"`

---

### Task 8: Run history

**Files:**
- Modify: `bin/job` (`record`)
- Modify: `lib/report.py` (parse `runs.tsv`)
- Modify: `lib/report.py` (HTML table gains duration + last 5 runs)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: `state/last`.
- Produces: `state/runs.tsv` `<start-iso>\t<end-iso>\t<rc>\t<verb>\t<unit>\t<seconds>`; `status --json` gains `runs` (last 10) and `last_duration`.

- [ ] **Step 1: Write the failing test**

```bash
reset
bin/job run noop >/dev/null 2>&1
bin/job run noop >/dev/null 2>&1
n=$(wc -l < jobs/noop/state/runs.tsv | tr -d ' ')
check "two runs recorded" 2 "$n"
```

- [ ] **Step 2: Run it and watch it fail** — no `runs.tsv`.

- [ ] **Step 3: Implement** — `record` gets the start timestamp from a `state/start` file written before exec; append one line per completed run.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: run history with durations"`

---

### Task 9: Resource locks

**Files:**
- Create: `lib/locks.sh`
- Modify: `jobs/noop/job.sh` (take `fixture` resource)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: `u_resource <name>` acquires `state/resources/<name>.lock` containing `pid<TAB>expiry-epoch`, reclaims an expired or dead pid, releases on EXIT; waits up to `U_RESOURCE_WAIT` seconds (default 0) then exits `0` held.

- [ ] **Step 1: Write the failing test**

```bash
reset
# hold the resource from a background shell, then run: the job must hold, not crash
( mkdir -p jobs/noop/state/resources; printf '%s\t%s\n' $$ $(( $(date +%s) + 60 )) > jobs/noop/state/resources/fixture.lock; sleep 3 ) &
sleep 1
bin/job run noop >/dev/null 2>&1; check "busy resource holds" 0 "$?"
wait
reset
( mkdir -p jobs/noop/state/resources; printf '999999\t%s\n' $(( $(date +%s) - 60 )) > jobs/noop/state/resources/fixture.lock ) 
bin/job run noop >/dev/null 2>&1; check "expired resource reclaimed" 0 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — the fixture ignores resources.

- [ ] **Step 3: Implement** — acquire/release with the same stale-pid logic as the run lock, plus expiry.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: cross-job resource locks"`

---

### Task 10: Asset approval gate

**Files:**
- Modify: `lib/gates.py` (add `manifest`)
- Create: `docs/asset-manifest.md`
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: `gates.py manifest --file shots.json --min 15`; the JSON is a list of `{shot, query, file, approved}`; passes only when every entry has `approved: true` and `file` exists and is non-empty; message names the first unapproved shot.

- [ ] **Step 1: Write the failing test**

```bash
printf '[{"shot":1,"query":"staff badge","file":"%s","approved":false}]' "$PWD/jobs/noop/out/hello.txt" > /tmp/shots.json
lib/gates.py manifest --file /tmp/shots.json --min 1; check "unapproved shot fails" 1 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — unknown subcommand.

- [ ] **Step 3: Implement** — argparse subcommand; read JSON; check count, approvals, files; report the first offender by shot number and query.

- [ ] **Step 4: Run the tests** — passes both ways (approved and not).

- [ ] **Step 5: Commit** `git commit -m "feat: asset approval manifest gate"`

---

### Task 11: Memory gate

**Files:**
- Modify: `lib/gates.py` (add `memory`)
- Modify: `lib/memory.sh` (`u_memory_check`; honour `U_PI_MEMORY`)
- Create: `tests/fake-pi-memory/{qmd-mem.sh,lint-memory.py,memory-pass-check.py}`
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: `U_PI_MEMORY`, `U_MEMORY_FILES`.
- Produces: `gates.py memory --files a.md,b.md` exits `0` only when the stub store's three scripts all exit `0`; `u_memory_pass` calls it instead of shelling the three commands inline.

- [ ] **Step 1: Write the failing test**

```bash
reset
U_MEMORY=1 U_PI_MEMORY="$PWD/tests/fake-pi-memory" FAKE_MEMORY_RC=1 bin/job run noop >/dev/null 2>&1
check "a failing memory pass blocks" 2 "$?"
reset
U_MEMORY=1 U_PI_MEMORY="$PWD/tests/fake-pi-memory" FAKE_MEMORY_RC=0 bin/job run noop >/dev/null 2>&1
check "a passing memory pass completes" 0 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — the real store is used, so the test either passes wrongly or hits `~/pi-memory`.

- [ ] **Step 3: Implement** — the fake store's three scripts exit `$FAKE_MEMORY_RC`; `gates.py memory` runs save → lint → receipt and reports which failed.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "test: memory pass is enforced and testable"`

---

### Task 12: Agent adapter

**Files:**
- Create: `lib/agent.sh`
- Create: `tests/fake-agent.sh`
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: `U_RUNNER` ∈ `dsh|claude|codex|fake`.
- Produces: `u_agent <prompt-file> <marker>` — runs the runner non-interactively, writes the marker only after the runner exits `0`, returns non-zero on runner failure; a second call with an existing marker returns `0` without invoking the runner.

- [ ] **Step 1: Write the failing test**

```bash
reset
calls=/tmp/lightsout-agent-calls; rm -f "$calls"
U_RUNNER=fake FAKE_AGENT_LOG="$calls" bin/job run noop >/dev/null 2>&1
bin/job run noop >/dev/null 2>&1
check "agent invoked once" 1 "$(wc -l < "$calls" | tr -d ' ')"
```

- [ ] **Step 2: Run it and watch it fail** — no adapter exists.

- [ ] **Step 3: Implement** — dispatch table; each runner gets its documented non-interactive invocation; the marker is `<prompt>.done` under `$STATE`.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: one agent wrapper for every runner"`

---

### Task 13: Scaffolder

**Files:**
- Create: `bin/scaffold`
- Modify: `spec/SCHEMA.md` (state the parsing rules the scaffolder enforces)
- Test: `tests/run.sh`

**Interfaces:**
- Consumes: a spec file with the headings `deliverable`, `cadence`, `unit`, `inputs`, `stages`, `guards`, `publish`, `memory`, `budget`; each stage line containing `gate:`.
- Produces: `jobs/<name>/job.sh` where every stage's gate is `u_gate never`, and `jobs/<name>/spec.md`; refuses (exit `64`) if a heading is missing or a stage has no `gate:`; `bin/job plan <name>` exits `0` and reports each unimplemented gate.

- [ ] **Step 1: Write the failing test**

```bash
rm -rf jobs/example
bin/scaffold spec/examples/digest.md >/dev/null; check "scaffold exits 0" 0 "$?"
[ -f jobs/example/job.sh ]; check "job emitted" 0 "$?"
bin/scaffold spec/examples/digest.md >/dev/null; check "scaffold refuses to overwrite" 64 "$?"
```

- [ ] **Step 2: Run it and watch it fail** — no `bin/scaffold`.

- [ ] **Step 3: Implement** — parse headings with awk; emit a job that calls `u_guard_*`, then one `u_gate never` per stage, then `u_memory_pass` and `u_done`; write only when the target does not exist.

- [ ] **Step 4: Run the tests** — passes.

- [ ] **Step 5: Commit** `git commit -m "feat: spec to job scaffolder"`

---

### Task 14: Port the video job

**Files:**
- Create: `jobs/everyday-laws/job.sh`
- Create: `jobs/everyday-laws/spec.md` (copy of `spec/examples/video.md`)
- Test: manual, then one overnight run

**Interfaces:**
- Consumes: the real driver at `../video-automation/pipeline/daily-build.sh` via `VIDEO_REPO` (default `$HOME/programming/video-automation`).
- Produces: `bin/job plan everyday-laws` prints the same episode the driver's preflight prints; `bin/job run everyday-laws` runs stage 1 through `u_agent` and stages 2–8 through the driver, with the master gated by duration/resolution/loudness.

- [ ] **Step 1: Write the failing test** — `bin/job plan everyday-laws` exits `64` (no such job).

- [ ] **Step 2: Implement** the job: `u_guard_day`, `u_guard_lock`, `u_guard_attempts`, `u_guard_disk 2000`, preflight parse, `u_agent` for stage 1, driver for stages 2–8, `u_gate duration/resolution/loudness` on the master, `u_memory_pass`, `u_done`.

- [ ] **Step 3: Verify with plan only**

Run: `bin/job plan everyday-laws`
Expected: prints the next episode number and every gate it would apply; writes nothing.

- [ ] **Step 4: One supervised real run**, comparing the master to the previous night's.

- [ ] **Step 5: Commit** `git commit -m "feat: port the everyday-laws video job"`

---

## Self-review notes

- Every gap in the table maps to a task; Task 1 exists only because Tasks 2, 3, 6 and 11 need the class.
- Task 14 depends on 12 and 10; it is last on purpose.
- The plan does not add a scheduler, a server, or a database, because the contract forbids all three.
