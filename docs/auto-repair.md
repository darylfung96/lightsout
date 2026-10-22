# Bounded auto-repair

A blocked job routes to one of four places — a bounded retry, a deterministic
repair, one agent attempt, or a human — and the policy decides which; the rest
of this page explains that route and its limits. The policy is small enough to
read in one sitting, and every attempt it authorises is bounded, so you can
predict what a scheduled run will do before you trust it.

Auto-repair is opt-in. Without a `triage.conf` or the `--auto-repair` flag,
`run` behaves exactly as it did before this feature existed, and every block
waits for a person, because auto-repair is opt-in.

## Why it exists

At 03:00 a failed job has only its files: a blocked line, a log, and whatever
artifacts the last stage wrote. The manager can read those files, so it can
retry a transient failure, recognise a small class of environmental failures
and fix them, and it can ask an agent for a narrow artifact edit when the cause
looks like a bad or missing file. It cannot invent a credential, decide that a
bad video is acceptable, or change the job's own code; those stops stay with a
human on purpose.

Think of the attempt as bounded: the manager tries the smallest fix it is
allowed to try and then re-runs the job, because only the job's gate can decide
whether the attempt worked. When the gate says no, the manager records what
happened and stops rather than trying variations until something turns green,
because retrying variations is how a wrong fix becomes expensive.

## Quick start

```sh
cd ~/my-office
cat > triage.conf <<'EOF'
# job              source          action
everyday-laws      gate:duration   agent:apply
everyday-laws      gate:sections   agent:propose
daily-video        network         retry
everyday-laws      credentials     human
*                  lock            repair:stale-lock
*                  *               human
EOF
lightsout triage everyday-laws       # show what the current block would route to
lightsout heal everyday-laws         # one attempt, in the policy's mode
lightsout run everyday-laws --auto-repair
```

`triage` writes nothing, so it is safe to run while a job is blocked. `heal`
performs one attempt and stops; `run --auto-repair` wraps a normal run in the
bounded loop and is the form a scheduler should call.

## The policy

Put `triage.conf` at the workspace root; the manager reads it there and never
rewrites it. Write three fields per line — job, source, and action; `#` starts a
comment, blank lines are ignored, and `*` is a wildcard.

| action | what it does |
|---|---|
| `human` | show the block and stop; the default when no rule matches |
| `retry` | wait, then re-run the job once; only for a `retryable` block |
| `repair:<name>` | run the `repair_<name>()` function from `lib/repairs.sh` |
| `agent:propose` | write a proposal and stop, for a human to approve |
| `agent:apply` | validate the proposal, apply it, and re-run the job |

Resolution tries four levels in order and stops at the first match: exact
`(job, source)` , then `(job, *)` , then `(*, source)` , then `(*, *)` ; the
exact rule wins because it is the most specific. A duplicate exact rule is a
config error, because the first match would otherwise be ambiguous. A malformed
file makes `triage` , `heal` , and `run --auto-repair` exit `64` with the line
number, while plain `run` , `status` , `report` , and `check` never read it.

The `source` field anchors everything else, because progress is measured by
comparing it between attempts. Jobs should pass a stable identifier to `u_block`
, for example `--source gate:duration` or `--source stage:3` . When a job leaves
the source `unknown` , the manager derives `gate:<name>` from a reason that
starts with `gate failed: <name>` ; a source that is still unknown cannot prove
progress, so the loop stops.

`LIGHTSOUT_TRIAGE=<path>` points the manager at a policy outside the workspace
root, which is mainly useful in tests.

## The retry action

`retry` is for a failure that is expected to clear by itself — an API that
returned a 503, a connection that dropped, a lock that was about to be
released. It changes no artifact: it waits `LIGHTSOUT_RETRY_BACKOFF` seconds
(default 30, maximum 600), re-runs the job, and lets the same gate decide
again. The wait is a duration, not a budget, so `0` is valid and useful in
tests.

`lightsout retry` is a different path: the verb runs an allowlisted repair and
then the job, while the policy action only re-runs the job, because a retry
must not pretend to have fixed an artifact. The policy action is for a world
that needs a second chance; `repair:<name>` is for a file or lock that needs to
change, and the agent modes are for an artifact that needs an edit.

The action is refused for a `judgment` or `human` block and falls back to a
human, because re-running cannot fix a verdict. A rerun that fails with the
same failure key stops the loop, so a persistent outage still ends with a
person. Each retry is recorded in the job's heal history with the action
`retry`; it counts against `MAX_AUTO_ATTEMPTS` but not against `MAX_REPAIRS` or
`MAX_HEALS`, because it spends no repair and no agent call.

## The two agent modes

In `agent:propose` the manager writes a proposal and stops, so a human can review the change before anything under `out/` moves.
The manager renders a prompt from `lib/prompts/repair.md` , runs the configured
runner once, stores the proposal under `state/heal/<date>/attempt-N/` , and
exits `2` . Nothing in `out/` changes until a person reviews the proposal and
runs:

```sh
lightsout heal everyday-laws --apply
```

In `agent:apply` the same proposal is applied without an approval step, so the
job can recover while nobody is watching. The same proposal is validated and
applied, and then the job re-runs immediately, so the gate still decides. It is
the default in the sample policy because the owner chose autonomous fixes; keep
a source on `agent:propose` while you are learning how that source fails.

## Which agent runs

The repair path uses the same agent profile as the job, because a proposal
written by one model should not be applied after a different model answered the
lesson. `jobs/<name>/agent.conf` names the runner and, for `dsh`, the patch that
pins provider, model, and effort; `agents.conf` can hold named profiles a job
selects with `profile = <name>`. The manager resolves that file once for a run
and once for a repair, so `run --auto-repair` and a hand-run `heal` take the same
path. Read [agent-step.md](agent-step.md) for the fields and precedence.

Because the pin has to stay visible after the call, `lightsout heal` prints the
resolved profile before the runner starts, and the attempt's `meta.env` and the
history's last column record it. A profile that names a `dsh` model without a
patch is a config error, because `dsh` has no `--model` flag; a missing patch is
a warning in `doctor` and a refusal at run time, so the manager never silently
falls back to a different model.

The validator is the boundary in both modes, because the prompt can be argued
with and a path check cannot. It accepts only relative paths under an allowlist,
refuses `job.sh` , `spec.md` , `state/` , `logs/` , `lib/` , and `spec/` ,
refuses symlinked path components, limits a proposal to 20 edits and 2 MiB of
text, and rejects unknown fields. An agent cannot smuggle in a shell command,
because the proposal format has no field for one.

After the attempt the manager compares the new block's failure key with the old
one, because that comparison is the only evidence of progress. A different key
means the failure moved, so the loop may try again; the same key means the fix
did not work, so it stops. The same key means the fix did not work, so the loop
stops even when the daily budget has room left.

## What the agent may edit

The default allowlist is `out/` , so a job has to ask for anything wider. A job
can widen it with `jobs/<name>/repair.allow` , one prefix per line:

```text
out/
assets/generated/
```

Every prefix is relative to `jobs/<name>/`; `..`, absolute paths, and symlinked
components are refused. The protected list is refused even when an allow entry
names it, because the repair must not be able to edit the thing that judges it.

Content edits are UTF-8 text. A binary artifact can be deleted so the job
fetches or renders it again, but the agent cannot write new binary bytes. The
asymmetry is deliberate, because deleting a stale take is a real repair while
inventing a video file is not, and the validator allows only the first.

## Caps and budgets

| setting | scope | default | counts |
|---|---|---|---|
| `MAX_HEALS` | one job per day | 2 | agent calls recorded in the job's history |
| `MAX_HEALS_TOTAL` | whole workspace per day | 5 | agent calls in `logs/heal/<date>.tsv` |
| `MAX_AUTO_ATTEMPTS` | one `run --auto-repair` invocation | 3 | repair and agent attempts in that loop |
| `MAX_REPAIRS` | one job per day | 2 | deterministic repairs |
| `U_AGENT_TIMEOUT` | one runner call | 600 | seconds before the runner is killed |

`LIGHTSOUT_RETRY_BACKOFF` sets the wait before a `retry` action (default 30
seconds, maximum 600); it is a duration, not a budget, so `0` is valid and
useful in tests.

The workspace ledger is reserved before the runner starts, because a call that
fails has already spent the money. A short lock serialises the read and
increment, so two jobs cannot both take the last slot. The counter is never
decremented, because a failed call has already spent the money.

A per-job `MAX_HEALS` stops one broken pipeline from consuming the shared
budget; `MAX_HEALS_TOTAL` bounds the bill for the whole office. Deterministic
repairs are local shell functions, so they count against `MAX_REPAIRS` rather
than the agent budget.

## What the state looks like

```text
jobs/<name>/state/heal/
  2026-10-21/
    attempt-1/
      prompt.md            rendered, redacted prompt
      runner.log           the runner's stdout and stderr
      proposal.json        what the agent returned
      meta.env             action, keys, verdict, rc, cost, resolved agent
      verdict              passed, no-progress, timeout, violation, ...
      before, after        the failure keys around the attempt
      backup/              originals of edited files, mirrored
      applied.tsv          JSON-quoted path + created/replaced, used by --revert
    attempt-2/
  history.tsv              one line per attempt; the last column is the agent
  lock/                    the job-level attempt lock
logs/heal/
  2026-10-21.tsv           one line per agent call across jobs
```

`status --json` adds `heal_attempts_today` and `last_heal` to each job row, and
`lightsout diagnose <job>` appends a "Repair attempts" section so a red row
explains what was already tried. A `retry` attempt has no prompt, proposal or
backup; it records the same `meta.env`, `before`, `after` and `verdict` files as
the other actions, which is enough to see whether the rerun moved.

## Applying and reverting

`lightsout heal <job> --apply` applies the newest stored proposal for today and
re-runs the job; it re-validates the stored JSON because the file may have been
edited between the attempt and the approval.

`lightsout heal <job> --revert N` restores the backup from `attempt-N`; a
replaced file comes back byte-for-byte and a created file is removed. The record
is `applied.tsv`, whose paths are JSON-quoted so a newline or tab cannot forge a
second record. A revert refuses a path outside the current `repair.allow`, because the
allowlist is what the apply validated. It also refuses an older attempt while a
newer applied attempt exists, because undoing the older one would discard the
newer edit.

`--revert` currently resolves today's attempt directory; a cross-day revert
needs a rule for which day wins when two attempts share a number. A cross-day
revert is a reasonable extension, but it needs a rule for which day wins when
two attempts share a number.

## Scheduling

The scheduler still calls `run`, with the new flag:

```sh
0 6 * * * cd ~/office && lightsout run daily-video --auto-repair
*/10 * * * * cd ~/office && lightsout check
```

`lightsout schedule daily-video --launchd --auto-repair` prints the plist with
that flag already in it, and the same flag on the cron form prints the line
above, so a regenerated trigger does not silently drop the loop.

`LIGHTSOUT_AUTO_REPAIR=1` enables the same loop for a scheduler that prefers an
environment variable. Exit codes are unchanged: `0` done or held, `1` failed,
`2` blocked. A blocked job that has exhausted its attempts notifies once when
the block was raised and then stays quiet while still exiting `2`.

`run_auto` compares the block's date with today's date before it repairs or
retries, so a block that survives to the next day records a failure that did
not clear; the next scheduled run stops with "block dated ..." and leaves it
for a human instead of retrying a stale failure every morning.

## When it does not repair

| verdict | meaning | what happens next |
|---|---|---|
| `human` | the policy routed it to a person, or no rule matched | status and one notification |
| `cap` | a job, workspace, or loop budget is exhausted | status; next day resets it |
| `unfixable` | the agent returned `fixable:false` | status with the agent's reason |
| `invalid:<rule>` | the proposal failed validation | status; nothing was written |
| `timeout` | the runner exceeded `U_AGENT_TIMEOUT` | status; the runner was killed |
| `runner-failed` | the runner exited non-zero | status with its exit code |
| `violation` | the runner changed a file during the attempt | status; the proposal was not applied |
| `apply-failed` | a write failed and the batch was rolled back | status; originals restored |
| `no-progress` | the same failure returned after the retry or fix | status; the applied files stay, and `--revert` can undo them |
| `failed` | the re-run exited non-zero without a new block | status with its exit code |
| `passed` | the re-run finished or was deliberately held | normal status |

A `no-progress` attempt keeps the files it wrote, because a partial fix can make
the next human decision easier. Use `--revert` when you want the original
artifact back.

## Limits

- The agent is not sandboxed by the manager. v1 takes a metadata fingerprint of
  the job and code directories before the runner and refuses the proposal when
  anything changed; that is a tripwire for accidental writes, not a security
  boundary against a determined process. Adapter-level read-only modes are the
  follow-up.
- Redaction is best-effort. The prompt scrubs common token shapes and secret
  values, but logs can contain data no scanner recognises; run the runner
  locally and keep secrets out of job logs.
- The policy is never learned. The manager does not add, remove, or promote
  rules; a human edits the file, and `doctor` validates it.
- An apply can trigger expensive work, because deleting a bad artifact may force
  a re-render. The caps bound the number of attempts, not the cost of one
  attempt; watch the job's spend ledger.
- The first real use is supervised. Run `triage` and a single `heal` by hand on
  a job before adding `--auto-repair` to its scheduler.

## FAQ

**Why did my job not repair?** Run `lightsout triage <job>`. It prints the action,
the matching rule, and the attempt counts, so the usual answers are visible: no
rule matched, the class is not `retryable` for a retry or repair, or a cap is
exhausted.

**Where is the proposal?** The `heal` output prints the absolute path, which is
`jobs/<name>/state/heal/<date>/attempt-N/proposal.json`.

**How do I turn it off?** Remove `--auto-repair` from the scheduler and the
policy entry; plain `run` is unchanged. The files from past attempts stay where
they are, because they are the record a human reads after a failure.

**How do I undo an applied fix?** Run `lightsout heal <job> --revert N` with the
attempt number from the history or the `heal` output.

**Can the agent edit `job.sh` ?** No. The validator refuses the path before
anything is written, because the proposal is data; `doctor` still expects a
human to own the job.