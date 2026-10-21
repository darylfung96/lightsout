# LightsOut: bounded auto-repair — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a blocked job route itself — to a deterministic repair, a bounded
agent attempt, or a human — and let a scheduled `run` perform the safe attempts
without a second command, while keeping the gate as the only thing that can
declare success.

**Architecture:** A human-edited `triage.conf` maps `(job, failure source)` to
one of four actions; the manager reads the file and never rewrites it. Two new
verbs separate seeing from acting, so `triage` prints the decision and `heal`
performs one attempt. `run --auto-repair` wraps the existing run in a bounded
loop that performs the action, re-runs the job, compares the new failure to the
old one, and stops on no progress or on the cap. An agent returns a JSON
proposal of file edits; the manager validates the paths, applies them atomically
with a backup, and re-runs the same gates. The agent never runs commands, never
touches `state/`, and never edits a gate, a spec, or `job.sh`, because the
proposal is data rather than an execution plan. State stays files; there is no
daemon, database, or new dependency.

**Tech Stack:** Rust 1.82+ (existing `serde_json` only), bash 3.2 (macOS system
shell), existing runners through `u_agent` (`dsh`, `claude`, `codex`, `fake`),
the existing 261-check contract suite.

**Spec:** `docs/run-contract.md` (the interface), `docs/design.md` (why),
`docs/agent-step.md` (the agent wrapper), and `lib/repairs.sh` (the repair
rules), which together define what a change must keep true.

---

## Decisions at a glance

| # | decision | rationale |
|---|---|---|
| D1 | Policy lives in workspace-root `triage.conf`, TSV, human-edited | one reviewable file, no schema parser, no new dependency; `lightsout.toml` stays a marker |
| D2 | Actions are `repair:<name>`, `agent:propose`, `agent:apply`, `human` | matches the three existing block classes and makes "who may attempt" explicit |
| D3 | Missing policy means every failure is `human` | fail closed; auto-repair is opt-in |
| D4 | New verbs are `triage` (read-only) and `heal` (one attempt); auto mode is `run --auto-repair` | `retry` keeps its existing meaning; no silent behavior change |
| D5 | Agent output is JSON edits, not commands and not a patch | deterministic validation, no shell execution, no diff parser |
| D6 | Edits are confined to `jobs/<name>/` prefixes from `repair.allow` (default `out/`) | the agent repairs artifacts, not code or state |
| D7 | Always refused: `job.sh`, `spec.md`, `state/blocked`, `state/done`, `lib/`, `spec/`, `.git/`, `lightsout.toml`, and the allow file itself | the repair must not be able to edit the checklist |
| D8 | Progress = the *failure key* changed; same key after an attempt stops the loop | otherwise a new error text resets the budget and the loop never ends |
| D9 | Caps: `MAX_HEALS` (2/day/job), `MAX_HEALS_TOTAL` (5/day/workspace), `MAX_AUTO_ATTEMPTS` (3/invocation), `U_AGENT_TIMEOUT` (600s) | the per-job cap stops one job hogging the budget, and the workspace cap bounds the shared bill; `MAX_REPAIRS` is untouched |
| D10 | `agent:apply` is the default in the sample policy and the `init` template; `agent:propose` is an explicit exception for a source that needs approval | the owner chose autonomous fixes, and propose stays available where an edit is too risky to apply unattended |
| D11 | The policy is never learned or rewritten by the manager | a system that can expand its own authority is the failure this repo exists to prevent |
| D12 | No changes to the 261 contract checks; new tests live outside the manifest | the existing spec stays intact while new behavior is tested |
| D13 | A workspace ledger at `logs/heal/<date>.tsv` counts every agent attempt across jobs | the per-job state stays under `jobs/`, while the shared bill has one workspace-wide counter |

## Non-goals

- No scheduler, daemon, server, or database. `launchd`/cron/`systemd` still decides *when*.
- No agent executing shell commands, no agent-run tests, no agent editing the manager.
- No self-modifying policy and no automatic switch between `propose` and `apply`.
- No change to `check`: an alarm still never repairs.
- No change to `retry`: the manual, deterministic path keeps its semantics.
- No provenance or approval UI; the files and `diagnosis.md` are the record.
- No automatic repair of `check`-detected stale runs; those stay a human alarm.

---

## Data formats

### `triage.conf`

Workspace root. Write one rule per line, whitespace-separated; `#` starts a
comment and blank lines are ignored.

```text
# job            source          action
*                lock            repair:stale-marker
*                credentials     human
everyday-laws    gate:duration   agent:apply
everyday-laws    gate:sections   agent:apply
# everyday-laws  gate:sections   agent:propose   # opt-in approval for this source
everyday-laws    *               human
*                *               human
```

- **job** — a directory name under `jobs/`, or `*`.
- **source** — the `source` field of `state/blocked`, or `*`. Sources must be
  stable identifiers; `gate:<name>` and `stage:<n>` are the convention. A job
  that passes a free-form source still works, but progress detection is only as
  good as the identifier.
- **action** — exactly one of:
  - `repair:<name>` — matches `[a-z0-9][a-z0-9-]*`, must name a
    `repair_<name>()` in `lib/repairs.sh`. Only valid for a `retryable` block;
    triage downgrades a mismatch to `human` and logs why.
  - `agent:apply` — the default; prepare, validate, apply, re-run.
  - `agent:propose` — prepare a proposal, do not apply, exit `2`; use it for
    a source where a human should approve the first fixes.
  - `human` — show the status and stop.

**Precedence**, first match wins:

1. exact `(job, source)`
2. `(job, *)`
3. `(*, source)`
4. `(*, *)`

A duplicate exact rule is a config error, because precedence would otherwise be
ambiguous. A rule for a job that does not exist is a `doctor` warning rather
than an error, so deleting a job does not break the whole policy file. Unknown
action, unknown `repair:` name, absolute path, or a line that is not 3 fields is
a config error.

Only `triage`, `heal`, and `run --auto-repair` read the policy; a malformed
file makes those three exit `64` and print the line number, while plain `run`,
`status`, `report`, and `check` never load it. `LIGHTSOUT_TRIAGE=<path>`
overrides the location (used by tests).

**Failure key** for progress comparison:

```text
source != "unknown"  ->  source
source == "unknown" and reason starts with "gate failed: <name> ..."  ->  "gate:<name>"
otherwise            ->  "unknown"
```

A key of `unknown` on either side counts as no progress, because the manager
cannot prove the failure changed.

### `jobs/<name>/repair.allow`

Optional. Write one path prefix per line, relative to `jobs/<name>/`; `#`
starts a comment. A missing file means exactly one entry, `out/`, so an absent
allow file still confines edits to artifacts.

```text
out/
assets/generated/
```

Every resolved edit must be under one of these prefixes, because the validator
refuses anything outside them. Prefixes are normalized lexically; `..`,
absolute paths, and symlinked components are refused. The protected list in D7
is refused even if an allow line names it.

### Proposal JSON

The agent writes this to the path named in the prompt (and may also print it):

```json
{
  "fixable": true,
  "summary": "Stage 3 failed because out/voice.wav is 48.0 s; the gate needs 55-62 s. Deleting the stale take forces the next run to synthesize a new one.",
  "edits": [
    { "path": "out/voice.wav", "delete": true },
    { "path": "out/lesson.md", "content": "..." }
  ]
}
```

Rules, enforced with `serde_json::Value` (no derive, no new dependency):

- unknown top-level keys are rejected (`deny_unknown_fields` behavior by hand);
- `fixable` is a bool; `false` requires `edits` to be empty and is a valid
  terminal outcome (record `unfixable`, exit `2`);
- `summary` is 1–2000 characters;
- `edits` has 0–20 entries, unique paths;
- each edit is either `{path, content}` or `{path, delete: true}`, not both;
- `content` is UTF-8 text, total across edits ≤ 2 MiB;
- `delete` targets an existing regular file.

### State layout

```text
jobs/<name>/state/heal/
  2026-10-21/
    attempt-1/
      prompt.md            rendered, redacted prompt
      runner.log           stdout+stderr of the runner
      proposal.json        validated only after it is copied here
      verdict              applied | passed | no-progress | unfixable |
                           invalid:<why> | refused:<why> | timeout |
                           apply-failed | interrupted | proposed | cap | human
      before               copy of state/blocked before the attempt
      after                copy of state/blocked after the re-run, or "done"
      backup/              originals of every edited path, mirrored
      meta.env             key=value: action, source, key, rc, seconds, cost
    attempt-2/
  history.tsv              one line per attempt, see below
  lock/                    manager-level loop lock (directory)
```

Workspace-level ledger:

```text
logs/heal/
  2026-10-21.tsv           one line per agent attempt across all jobs
  lock/                    short-lived budget lock (60 s TTL)
```

`history.tsv`:

```text
start-iso  job  attempt  action  before-key  after-key  verdict  rc  seconds  cost
```

`cost` is empty when the runner does not report one, so a missing figure never
blocks an attempt.

### Prompt template

The template is committed at `lib/prompts/repair.md` and rendered by simple
`{{KEY}}` replacement, which keeps the prompt reviewable instead of improvised.
Slots:

| slot | content |
|---|---|
| `{{JOB}}`, `{{UNIT}}` | names |
| `{{SPEC}}` | `spec.md`, capped at 8 KiB |
| `{{BLOCKED}}` | the four-field blocked line |
| `{{DIAGNOSIS}}` | `state/diagnosis.md` if present, else "not generated" |
| `{{LOG_TAIL}}` | last 80 lines of the run log, capped at 32 KiB, redacted |
| `{{ATTEMPTS}}` | one summary line per prior attempt, with verdict and key |
| `{{ALLOWED}}` | the resolved allow prefixes |
| `{{PROPOSAL_PATH}}` | absolute path the agent must write |
| `{{SCHEMA}}` | the JSON schema above, as text |

The template states, in this order: the job is blocked; your only output is the
JSON; you cannot run commands; you may only edit paths under `{{ALLOWED}}`; you
must not edit `job.sh`, `spec.md`, `state/`, or anything that judges the
work; if you are not confident, return `fixable:false` with a reason. Untrusted
content (log, diagnosis, spec) is wrapped in `<untrusted-data>` tags, so the
model is told which text is data. The validator, not the prompt, is the security
boundary, because a prompt can be argued with and a path check cannot.

---

## Algorithms

### Triage resolution

```text
fn triage(policy, job, blocked) -> Decision:
    if blocked.class == "retryable" and action is agent:*:
        warn "an allowlisted repair exists; agent action is unusual"
    if action is repair:<name> and blocked.class != "retryable":
        return Decision::human("class <c> is not retryable; repair refused")
    return Decision { action, matched_rule, key: failure_key(blocked) }
```

`triage <job>` prints the decision and exits `0`; with `--json` it prints
`{job, blocked:{class,source,reason,iso}, action, mode, matched_rule,
heals_today, max_heals, workspace_heals_today, max_heals_total, key}`. It
writes nothing.

### `run --auto-repair`

Also enabled by `LIGHTSOUT_AUTO_REPAIR=1`. Plain `run` is unchanged, so a
scheduler that does not opt in sees the 0.1.0 behavior.

```text
fn run_auto(repo, name):
    lock = acquire(state/heal/lock, ttl=3600) or return 0 held
    attempts = 0
    blocked = read_blocked()
    if blocked and blocked.iso.date == today:
        return heal_loop(blocked, attempts, lock)   # heal yesterday's stop today
    if blocked and blocked.iso.date != today:
        print "block dated <date>; use: lightsout heal <job>"
        return 2
    rc = run_job(retrying=false)
    if rc != 2: return rc
    return heal_loop(read_blocked(), attempts, lock)

fn heal_loop(blocked, attempts, lock):
    while true:
        if attempts >= MAX_AUTO_ATTEMPTS:
            record("cap"); return 2
        decision = triage(blocked)
        if decision.action == human:
            record("human"); return 2
        if decision.action == repair(name):
            if repairs_today >= MAX_REPAIRS: record("cap"); return 2
            run_repair(name) or record("refused"); return 2
        if decision.action == agent_propose:
            if heals_today >= MAX_HEALS: record("cap"); return 2
            if workspace_heals_today >= MAX_HEALS_TOTAL: record("cap"); return 2
            reserve_workspace_heal()
            proposal = call_agent(blocked)          # may be unfixable/timeout
            record(proposal.verdict)
            print proposal path and "lightsout heal <job> --apply"
            return 2
        if decision.action == agent_apply:
            if heals_today >= MAX_HEALS: record("cap"); return 2
            if workspace_heals_today >= MAX_HEALS_TOTAL: record("cap"); return 2
            reserve_workspace_heal()
            proposal = call_agent(blocked)
            if not proposal.valid: record(...); return 2
            applied = apply_with_backup(proposal)
            if not applied: record("apply-failed"); return 2
        attempts += 1
        rc = run_job(retrying=true)
        if rc != 2: record("passed", rc); return rc
        next = read_blocked()
        if failure_key(next) == failure_key(blocked):
            record("no-progress"); return 2
        blocked = next
```

Notes:

- The repair branch reuses the existing `retry` internals; refactor
  `commands::retry` into `run_repair(paths, name) -> bool` plus the verb so both
  paths share the cap and the allowlist parsing.
- `apply_with_backup` is atomic per file and rolls back the files it already
  wrote if a later edit fails.
- A run that returns `0` but leaves `state/blocked` (a held run) records
  `passed` with `rc=0` and a note; the block stands.
- The loop never calls `check` and never notifies; the block's original
  notification already fired. Escalation adds the attempt history to
  `diagnosis.md`, not a second page.
- The workspace ledger is reserved before the runner starts, because the call
  spends money even when it fails; a short workspace lock serializes the
  read-increment and re-checks the cap, and the counter is never decremented.

### Validation

Run in this order; the first failure wins:

1. `proposal.json` parses, ≤ 1 MiB, top-level keys exact, `summary` bounds.
2. `fixable:false` → stop, `unfixable`, no edits allowed.
3. `edits` length 1–20, unique paths, no duplicates, total content ≤ 2 MiB.
4. For each path: not empty, not absolute, no `\`, no NUL, no component that is
   `.` or `..`, and lexical normalization does not escape `jobs/<name>/`.
5. Each existing component is checked with `symlink_metadata`; any symlink is
   refused (prevents a symlinked `out/` from escaping the job).
6. The normalized path starts with an allowed prefix from `repair.allow`.
7. The path is not in the protected set: `job.sh`, `spec.md`, `logs/`, `state/`,
   `lib/`, `spec/`, `lightsout.toml`, `triage.conf`, `repair.allow`, `.git/`,
   or any path whose first component is `.`.
8. `delete` targets an existing regular file; `content` targets a path whose
   parent exists and is a writable directory.
9. On success, copy the proposal to `attempt-N/proposal.json` (the runner's
   copy is untrusted input) and return the edit list.

Refusals record `refused:<rule>` and change nothing, because a refused proposal
must leave the workspace exactly as it was.

### Apply and rollback

```text
for edit in edits (sorted by path):
    if exists and not backed up: copy bytes + mode to attempt-N/backup/<path>
    if delete: remove_file
    else: write .<name>.tmp.<pid> in the same dir, fsync, rename over target
on any error:
    restore every backed-up file, remove every created file
    record apply-failed; exit 2
```

The applier never changes permissions or executable bits, because content is the
only thing a repair may touch. Directories are never created outside the job's
allowed roots; if the parent is missing, that is `refused:parent`.

### Progress and escalation

- Same key → `no-progress`, stop immediately, keep the applied files and the
  backup; the red row stays. `lightsout heal <job> --revert <n>` restores.
- Different key → one more attempt, still under the same `MAX_AUTO_ATTEMPTS`,
  `MAX_HEALS`, and `MAX_HEALS_TOTAL` budget.
- `unknown` on either side → `no-progress`.
- At the cap, `diagnosis.md` gains a "Repair attempts" section: each attempt's
  action, verdict, key before/after, diffstat, cost, and the backup path.
- The job stays blocked; `unblock` is still the only way a human clears it.

### Lock and interruption

- `state/heal/lock/` is created with `create_dir` (atomic). It holds `pid` and
  `expiry`; TTL 3600 s, refreshed before each agent call. A stale directory
  (expiry passed) is removed once and recreated. A live lock makes
  `run --auto-repair` exit `0` held with a log line.
- At loop start, any attempt directory without `verdict` is marked
  `interrupted` and the loop stops with exit `2`. `heal --force` may proceed
  manually; auto mode never guesses what a killed attempt did.
- The lock is released by a guard on every return path.

### Redaction

Before the prompt is written, scrub the log tail and diagnosis with
hand-rolled scanners (no `regex` dependency):

- `sk-`, `ghp_`, `github_pat_`, `xox[baprs]-`, `AKIA` token shapes;
- `Bearer <token>`;
- `(secret|token|password|api[_-]?key)\s*[:=]\s*<value>`;
- values of environment variables whose names match
  `*TOKEN*|*SECRET*|*KEY*|*PASSWORD*`.

Write `[redacted]` in place of each match, count the replacements in `meta.env`
, and record that the prompt was redacted. This is best-effort;
`docs/auto-repair.md` says logs may still contain sensitive data and the runner
is local.

---

## Security model

| threat | defense |
|---|---|
| agent edits the gate, spec, or `job.sh` to force a pass | protected-path refusal; validator runs before apply; a gate is still the only judge |
| path traversal or symlink escape | lexical normalization, no `..`, no absolute paths, `symlink_metadata` on every existing component |
| agent runs shell commands | the contract has no command field; unknown keys are rejected; the prompt forbids it; v2 read-only adapter |
| agent writes directly during the run (real runners) | pre/post fingerprint of `jobs/<name>/`, `lib/`, `spec/`, `triage.conf`; unexpected changes → `violation`, no apply, exit 2 (see Containment) |
| secrets in logs reach the model | redaction pass; local runner; documented limitation |
| prompt injection in artifacts or logs | untrusted-data tags; validator is the boundary, not the prompt |
| runaway cost or time | `MAX_HEALS`, `MAX_HEALS_TOTAL`, `MAX_AUTO_ATTEMPTS`, `U_AGENT_TIMEOUT`, optional `MAX_HEAL_COST`, per-attempt history |
| two schedulers healing one job | manager-level `state/heal/lock` |
| a crash mid-apply | atomic per-file writes, backup, rollback, `interrupted` marker |
| policy widening its own authority | policy is read-only config, edited by a human, validated by `doctor` |

### Containment (v1 vs v2)

`u_agent` runs the configured runner with whatever tools it has, so a prompt
that says 'do not edit files' is not a sandbox. A prompt that says "do not edit
files" is not a sandbox. v1 ships a tripwire:

- before the runner, record `(path, size, mtime)` for every file under
  `jobs/<name>/` except `state/heal/`, plus `lib/`, `spec/`, `triage.conf`;
- after the runner, compare; any change outside the proposal's paths is a
  `violation`, the proposal is not applied, and the job stays blocked;
- the tripwire detects accidental writes, not a determined agent; say so in the
  docs.

v2, tracked as a follow-up, adds adapter-level read-only modes
(`U_HEAL_READONLY=1`; a dsh patch without write tools, a claude permission mode,
a codex sandbox flag). Until an adapter can guarantee it, `doctor` reports
"containment: tripwire only" for that runner.

---

## Tasks

### Task 1: Policy parser and read-only `triage`

**Files:**
- Create: `src/triage.rs`
- Modify: `src/main.rs` (module list, dispatch, `USAGE`)
- Modify: `src/workspace.rs` (`doctor` validation)
- Create: `tests/auto_repair.rs`
- Modify: `docs/run-contract.md` (verb table, policy format)

**Interfaces:**
- `triage::Policy::load(path) -> Result<Policy, PolicyError>`
- `Policy::resolve(job, source) -> Resolution { action, matched_rule }`
- `triage::failure_key(blocked: &state::Blocked) -> String`
- `triage::run(repo, argv) -> i32` for the verb

- [ ] **Step 1: Write the failing tests** in `tests/auto_repair.rs`:
  - exact rule beats `(job,*)`, which beats `(*,source)`, which beats `(*,*)`;
  - a duplicate exact rule is an error with a line number;
  - an unknown action and an unknown `repair:` name are errors;
  - `triage` on a blocked job prints the action and leaves the tree unchanged
    (hash every file before/after).
- [ ] **Step 2: Run** `cargo test --test auto_repair`; expect compile failure /
  unknown verb.
- [ ] **Step 3: Implement** the parser (split on whitespace, `#` comments,
  line numbers in errors), the precedence table, `failure_key`, the verb and
  its `--json`, and the `doctor` checks.
- [ ] **Step 4: Run** `cargo test --locked` and `tests/run.sh`; both green,
  261 checks unchanged.
- [ ] **Step 5: Commit** `feat: triage policy and read-only triage verb`

### Task 2: Manager loop lock, history, and `run --auto-repair` (repair only)

**Files:**
- Create: `src/heal.rs` (lock, attempt dirs, history, loop skeleton)
- Modify: `src/commands.rs` (extract `run_repair`, add `--auto-repair` parsing)
- Modify: `src/main.rs` (dispatch)
- Modify: `src/state.rs` (read/write helpers if needed)
- Modify: `tests/auto_repair.rs`

**Interfaces:**
- `heal::Lock::acquire(state) -> LockResult`
- `heal::history_append(state, line)`
- `commands::run_auto(repo, argv) -> i32`
- `commands::run_repair(paths, name) -> bool`

- [ ] **Step 1: Failing tests**: a retryable block with a `repair:` rule is
  repaired and re-run to success by `run --auto-repair`; the same failure after
  the repair stops the loop; a second concurrent auto run exits `0` held; the
  history has one line per attempt.
- [ ] **Step 2: Run** and watch it fail.
- [ ] **Step 3: Implement** the lock directory (TTL, stale reclaim, guard),
  attempt dirs + `history.tsv`, the loop for `repair` only, `human` stop, cap
  and no-progress rules. Treat `agent:apply` as `human` until Task 6;
  `agent:propose` arrives in Task 3.
- [ ] **Step 4: Run** `cargo test --locked`, `tests/run.sh`, and
  `tests/install.sh`.
- [ ] **Step 5: Commit** `feat: bounded auto-repair loop for allowlisted repairs`

### Task 3: `heal` propose mode — prompt, runner, proposal capture

**Files:**
- Create: `lib/heal.sh` (`u_heal_agent` wrapper, timeout, capture)
- Create: `lib/prompts/repair.md`
- Modify: `src/heal.rs` (render prompt, call runner, read proposal)
- Modify: `src/main.rs` (dispatch `heal`)
- Modify: `tests/fake-agent.sh` (proposal modes; defaults unchanged)
- Modify: `tests/auto_repair.rs`

**Interfaces:**
- `heal::render_prompt(template, context) -> String`
- `heal::call_agent(paths, attempt_dir, timeout) -> AgentOutcome`
  (`Proposal(Value)`, `NoProposal`, `Timeout`, `RunnerFailed`)
- `lightsout heal <job>` runs one attempt; this task implements propose only,
  which stops after the proposal and prints its path. Apply lands in Task 5 and
  is wired into the loop in Task 6.

- [ ] **Step 1: Failing tests**: propose mode writes `proposal.json` and
  changes no artifact; `fixable:false` exits `2`; a timeout is recorded as
  `timeout`; the prompt has the schema, the allowed paths, and a redacted
  `sk-...` token.
- [ ] **Step 2: Run** and watch it fail.
- [ ] **Step 3: Implement** the renderer, the shell wrapper (timeout + capture,
  no `wait -n`, bash 3.2 compatible), fake-runner modes
  (`FAKE_AGENT_PROPOSAL`, `FAKE_AGENT_RC`, `FAKE_AGENT_SLEEP`), and the
  `heal` verb.
- [ ] **Step 4: Run** the full suites.
- [ ] **Step 5: Commit** `feat: heal propose mode with a rendered repair prompt`

### Task 4: Proposal validation and refusal

**Files:**
- Modify: `src/heal.rs`
- Modify: `tests/auto_repair.rs`
- Create unit tests in `src/heal.rs`

**Interfaces:**
- `heal::validate(job_dir, allow: &[PathBuf], proposal: &Value) -> Result<Vec<Edit>, Refusal>`

- [ ] **Step 1: Failing tests** (table-driven): `../escape`, absolute path,
  `lib/g.sh`, `job.sh`, `state/blocked`, `triage.conf`, a symlinked component,
  duplicate paths, 21 edits, 3 MiB content, unknown key, `fixable:false` with
  edits — each refused with the precise rule and no file changed.
- [ ] **Step 2: Run** and watch it fail.
- [ ] **Step 3: Implement** the nine ordered checks from **Validation** above.
- [ ] **Step 4: Run** the full suites.
- [ ] **Step 5: Commit** `feat: refuse agent edits outside the artifact allowlist`

### Task 5: Apply, backup, rollback

**Files:**
- Modify: `src/heal.rs`
- Modify: `tests/auto_repair.rs`

**Interfaces:**
- `heal::apply(attempt_dir, job_dir, edits) -> Result<Applied, ApplyError>`
- `heal::revert(attempt_dir, job_dir) -> Result<(), ApplyError>`

- [ ] **Step 1: Failing tests**: a two-file apply writes both and backs both
  up; a read-only second target rolls the first back and records
  `apply-failed`; `revert` restores the originals byte-for-byte and removes
  created files.
- [ ] **Step 2: Run** and watch it fail.
- [ ] **Step 3: Implement** atomic temp+rename, backup copying, rollback, and
  `heal --revert <n>`.
- [ ] **Step 4: Run** the full suites.
- [ ] **Step 5: Commit** `feat: atomic apply and rollback for agent repairs`

### Task 6: Apply mode, progress, escalation

**Files:**
- Modify: `src/heal.rs` (loop with agent branches)
- Modify: `src/commands.rs`
- Modify: `src/diagnose.rs` (attempt history section)
- Modify: `src/status.rs` (`heal_attempts_today`, `last_heal` in JSON; HTML column)
- Modify: `tests/auto_repair.rs`

- [ ] **Step 1: Failing tests**: apply mode fixes the artifact and exits `0`;
  the same key after apply stops without a second attempt; a changed key allows
  a second attempt; `MAX_HEALS=2` stops the third and `MAX_HEALS_TOTAL=1`
  stops the second job; a `violation` from the containment tripwire applies
  nothing; `diagnosis.md` lists both attempts.
- [ ] **Step 2: Run** and watch it fail.
- [ ] **Step 3: Implement** the agent branches in `heal_loop`, the tripwire,
  the escalation section, and the status fields.
- [ ] **Step 4: Run** the full suites.
- [ ] **Step 5: Commit** `feat: apply agent repairs with progress and escalation`

### Task 7: Init templates, doctor, help, docs

**Files:**
- Modify: `src/workspace.rs` (`triage.conf`, `lib/heal.sh`, `lib/prompts/repair.md`
  in `templates()`; `doctor` policy/runner/allow checks)
- Modify: `src/main.rs` / `src/commands.rs` (`USAGE`, verb help)
- Create: `docs/auto-repair.md`
- Modify: `docs/run-contract.md`, `docs/install.md`, `README.md`, `AGENTS.md`
- Modify: `notes/10-active.md`, `notes/60-log.md`, `notes/05-standing-decisions.md`
- Modify: `Cargo.toml` `include` (`lib/prompts/**/*`, `triage.conf` if packaged)
- Modify: `tests/workspace_cli.rs`

- [ ] **Step 1: Failing tests**: `init` writes `triage.conf`, `lib/heal.sh`,
  and the prompt; `doctor` errors on a malformed policy and warns on a rule for
  a missing job; `doctor` reports "tripwire only" for a runner without
  containment.
- [ ] **Step 2: Run** and watch it fail.
- [ ] **Step 3: Implement** templates, doctor, help text, and the docs. The
  new docs page must say plainly: what it does, what it never does, the policy
  grammar, the two modes, the caps, the revert command, and the containment
  limitation.
- [ ] **Step 4: Run** `cargo test --locked`, `tests/run.sh`,
  `tests/install.sh`, `cargo package --list` (no warnings).
- [ ] **Step 5: Commit** `docs: document bounded auto-repair and its limits`

### Task 8: Release 0.2.0

**Files:**
- Modify: `Cargo.toml` (`version = "0.2.0"`)
- Modify: `src/repo.rs` (`VERSION`)
- Modify: `install.sh` (pinned default)
- Modify: `packaging/homebrew/lightsout.rb` (tag + sha)
- Modify: `.github/workflows/release.yml` (tag check only; no new asset)
- Modify: `docs/install.md`, `README.md`, `notes/60-log.md`

- [ ] **Step 1: Bump** the version and run the full local suites.
- [ ] **Step 2: Tag** `v0.2.0`, push, watch the release workflow; verify all
  four checksums and the installer.
- [ ] **Step 3: Update** the formula's `url`/`sha256`, push the tap, run
  `brew audit --strict --online`.
- [ ] **Step 4: Commit** `release: 0.2.0 with bounded auto-repair`.

---

## Test matrix

| # | scenario | expected |
|---|---|---|
| 1 | exact/star precedence | exact wins, first match, reported in `triage` |
| 2 | malformed policy | `triage`/`heal`/auto exit `64`, line number, plain `run` unaffected |
| 3 | `triage` read-only | no file changes, exit `0` |
| 4 | no policy file | action `human`, auto run exits `2`, no agent call |
| 5 | retryable + `repair:` | repair runs, gate passes, exit `0`, one history line |
| 6 | repair fails same key | no second repair, exit `2` |
| 7 | `agent:propose` | proposal written, no artifact change, exit `2` |
| 8 | `agent:apply` happy | edit applied, gate passes, exit `0`, backup exists |
| 9 | invalid path / protected file | refused, nothing written, verdict names the rule |
| 10 | `fixable:false` | `unfixable`, exit `2`, no edits |
| 11 | same key after apply | stop, `no-progress`, no second agent call |
| 12 | changed key after apply | second attempt allowed, then pass |
| 13 | `MAX_HEALS=2` | third attempt not made, exit `2`, two history lines |
| 14 | timeout | killed, `timeout`, exit `2` |
| 15 | apply failure mid-list | rollback, `apply-failed`, originals intact |
| 16 | interrupted attempt | next auto run refuses, exit `2` |
| 17 | concurrent auto runs | second exits `0` held |
| 18 | `heal --revert` | originals restored exactly |
| 19 | redaction | fake token absent from `prompt.md`, count recorded |
| 20 | containment tripwire | unexpected `job.sh` write → `violation`, no apply |
| 21 | plain `retry` regression | existing behavior and caps unchanged |
| 22 | `doctor` policy checks | errors/warnings as specified |
| 23 | `init` templates | new files present, `doctor` clean |
| 24 | existing contract | 261 checks and `cargo test --locked` green, manifest untouched |
| 25 | workspace heal budget | with `MAX_HEALS_TOTAL=1`, a second job's agent action is capped before the runner starts |

New tests live in `tests/auto_repair.rs` and Rust unit tests, not in
`tests/check_manifest.rs`. `tests/run.sh` and the manifest stay at 261.

---

## Rollout and compatibility

- **Opt-in.** Without `--auto-repair` / `LIGHTSOUT_AUTO_REPAIR=1` and without a
  `triage.conf`, behavior is byte-identical to 0.1.0.
- **`retry` unchanged.** The manual path keeps its class check, registry check,
  and `MAX_REPAIRS` cap.
- **Schema stays 1.** `triage.conf`, `lib/heal.sh`, and the prompt are additive
  files; a 0.1.0 workspace without them still runs (auto mode says "no policy").
- **Marker manager version** becomes 0.2.0 for new workspaces; old markers are
  still readable.
- **Migration:** `lightsout init` in an existing workspace does not overwrite;
  `lightsout doctor` names the missing files and the sample policy. A user adds
  `triage.conf` by hand and starts with all `human`, then promotes one source to
  `agent:apply` (or `agent:propose` where approval is wanted).
- **Rollback:** remove `--auto-repair` from the scheduler; nothing else changes.
  An applied agent edit can be reverted with `heal --revert`.

## Open questions for the owner

Resolved: D10 now defaults to `agent:apply` per the owner's choice.

1. **Caps:** are `MAX_HEALS=2` per job, `MAX_HEALS_TOTAL=5` per workspace, and
   `MAX_AUTO_ATTEMPTS=3` per invocation the right defaults?
2. **Allowed roots:** is `out/` the right default, with `repair.allow` for more?
3. **Name:** `heal` and `triage` — or `fix` and `route`?
4. **Opt-in duration:** keep `--auto-repair` explicit through 0.2.0 and consider
   default-on in 0.3.0 after the tests have run on real jobs?

## Definition of done

- All 25 matrix rows pass; the 261 contract checks and `tests/run.sh` are
  unchanged and green; `tests/install.sh` is green.
- `cargo test --locked`, `cargo build --release`, `cargo package --list` clean.
- `tests/auto_repair.rs` proves the dangerous cases: protected paths, same-key
  stop, cap, timeout, interruption, rollback, and concurrent runs.
- `docs/auto-repair.md` states the limitations: tripwire containment, redaction
  is best-effort, and the policy is never learned.
- The first real use is supervised: run `lightsout triage <job>` and
  `lightsout heal <job>` by hand on the `everyday-laws` job before adding
  `--auto-repair` to its scheduler.
